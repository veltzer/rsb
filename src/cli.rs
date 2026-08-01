use anyhow::{Result, bail};
use clap::{Args, CommandFactory, FromArgMatches, Parser, Subcommand, ValueEnum};
use clap_complete::{generate, Shell};
use std::str::FromStr;

#[derive(Parser)]
#[command(name = "rsconstruct")]
#[command(version = concat!(env!("CARGO_PKG_VERSION")))]
#[command(about = "Rust Build Tool - Incremental build system with templates", long_about = None)]
pub struct Cli {
    /// Show skip/restore/cache messages during build
    #[arg(short, long, global = true)]
    pub verbose: bool,

    /// What to show for output files (none, basename, path)
    #[arg(short = 'O', long, global = true, value_enum, default_value = "none")]
    pub output_display: OutputDisplay,

    /// What to show for input files (none, source, all)
    #[arg(short = 'I', long, global = true, value_enum, default_value = "source")]
    pub input_display: InputDisplay,

    /// Path format for displayed files (basename, path)
    #[arg(short = 'P', long, global = true, value_enum, default_value = "path")]
    pub path_format: PathFormat,

    /// Print each child process command before it is executed
    #[arg(long, global = true)]
    pub show_child_processes: bool,

    /// Show tool output even on success (default: only show on failure)
    #[arg(long, global = true)]
    pub show_output: bool,

    /// Output in JSON Lines format (machine-readable)
    #[arg(long, global = true)]
    pub json: bool,

    /// Suppress all output except errors (useful for CI)
    #[arg(short, long, global = true)]
    pub quiet: bool,

    /// Show build phase messages (discover, add_dependencies, etc.)
    #[arg(long, global = true)]
    pub phases: bool,

    /// Print graph size (product and edge counts) at each major build stage
    #[arg(long, global = true)]
    pub graph_stats: bool,

    /// Disable persistent mtime checksum cache (useful for CI/CD where the
    /// cache won't survive the build and the write overhead isn't worth it)
    #[arg(long, global = true)]
    pub no_mtime_cache: bool,

    /// When to use ANSI color output: auto (tty only), always, or never.
    /// Also honored via the NO_COLOR env var (sets mode to never).
    #[arg(long, global = true, value_enum, default_value = "auto")]
    pub color: ColorMode,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum ColorMode {
    /// Enable color if stdout is a tty and NO_COLOR is not set
    Auto,
    /// Always emit ANSI color escapes
    Always,
    /// Never emit ANSI color escapes
    Never,
}

impl Cli {
    /// Get the display options from CLI arguments
    pub const fn display_options(&self) -> DisplayOptions {
        DisplayOptions {
            output: self.output_display,
            input: self.input_display,
            path_format: self.path_format,
        }
    }
}

/// Output format for the dependency graph
#[derive(Debug, Clone, Copy, Default, ValueEnum)]
pub enum GraphFormat {
    /// DOT format (Graphviz)
    Dot,
    /// Mermaid diagram format (Markdown-friendly)
    Mermaid,
    /// JSON format (machine-readable)
    Json,
    /// Plain text hierarchical view
    Text,
    /// SVG format (requires Graphviz dot)
    #[default]
    Svg,
}

/// Viewer for opening the graph
#[derive(Debug, Clone, Copy, Default, ValueEnum)]
pub enum GraphViewer {
    /// Open as HTML with Mermaid in browser (no dependencies)
    Mermaid,
    /// Generate and open SVG using Graphviz dot
    #[default]
    Svg,
}

/// Build phases that can be stopped after
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
pub enum BuildPhase {
    /// Stop after discovering products (before dependency scanning)
    Discover,
    /// Stop after adding dependencies (before resolving graph)
    AddDependencies,
    /// Stop after resolving the dependency graph (before execution)
    Resolve,
    /// Stop after classifying products (show skip/restore/build counts)
    Classify,
    /// Run the full build (default)
    #[default]
    Build,
}

/// What to show for output files in build messages
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
pub enum OutputDisplay {
    /// Don't show output files
    #[default]
    None,
    /// Show only the filename (e.g., "main.elf")
    Basename,
    /// Show full relative path (e.g., "out/cc_single_file/main.elf")
    Path,
}

/// What to show for input files in build messages
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
pub enum InputDisplay {
    /// Don't show input files
    None,
    /// Show only the primary source file (first input)
    #[default]
    Source,
    /// Show all input files including headers/dependencies
    All,
}

/// Path format for displayed files
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
pub enum PathFormat {
    /// Show only the filename (e.g., "main.c")
    Basename,
    /// Show full relative path (e.g., "src/main.c")
    #[default]
    Path,
}

/// Display options for product output in build messages
#[derive(Debug, Clone, Copy)]
pub struct DisplayOptions {
    pub output: OutputDisplay,
    pub input: InputDisplay,
    pub path_format: PathFormat,
}

impl Default for DisplayOptions {
    fn default() -> Self {
        Self {
            output: OutputDisplay::None,
            input: InputDisplay::Source,
            path_format: PathFormat::Path,
        }
    }
}

impl DisplayOptions {
    /// Minimal display: just input source basename
    pub const fn minimal() -> Self {
        Self {
            output: OutputDisplay::None,
            input: InputDisplay::Source,
            path_format: PathFormat::Basename,
        }
    }
}

// Subcommand variants are kept in alphabetical order by their display name (kebab-case
// of the variant). Clap renders subcommands in declaration order, so this list IS the
// help output. Always insert new variants in alphabetical position.
#[derive(Subcommand)]
pub enum Commands {
    /// Manage dependency analyzers
    Analyzers {
        #[command(subcommand)]
        action: AnalyzersAction,
    },
    /// Execute an incremental build
    Build {
        /// Force rebuild even if files haven't changed
        #[arg(short, long)]
        force: bool,

        /// Show what would be built without executing anything
        #[arg(short = 'n', long)]
        dry_run: bool,

        /// Verify tool versions against .tools.versions before building
        #[arg(long)]
        verify_tool_versions: bool,

        /// Stop after a specific build phase
        #[arg(long, value_enum, default_value = "build")]
        stop_after: BuildPhase,

        #[command(flatten)]
        shared: SharedBuildArgs,
    },
    /// Manage the build cache
    Cache {
        #[command(subcommand)]
        action: CacheAction,
    },
    /// Clean build artifacts
    Clean {
        #[command(subcommand)]
        action: Option<CleanAction>,
    },
    /// Generate shell completion scripts
    Complete {
        /// The shells to generate completions for (if none specified, uses config file)
        #[arg(value_enum)]
        shells: Vec<Shell>,
    },
    /// Check build environment (requires config)
    Doctor,
    /// List all exit codes and their meanings (no config needed)
    Errors,
    /// Run fixers on source files (auto-format, auto-fix lint issues)
    Fix {
        #[command(subcommand)]
        action: FixAction,
    },
    /// Inspect built-in template functions exposed to Tera templates (no config needed)
    Functions {
        #[command(subcommand)]
        action: FunctionsAction,
    },
    /// Display the build dependency graph
    Graph {
        #[command(subcommand)]
        action: GraphAction,
    },
    /// Show project information
    Info {
        #[command(subcommand)]
        action: InfoAction,
    },
    /// Initialize a new rsconstruct project (no config needed)
    Init,
    /// Inspect rsconstruct phases and registered hooks (no config needed)
    Phases {
        #[command(subcommand)]
        action: PhasesAction,
    },
    /// Manage processors
    Processors {
        #[command(subcommand)]
        action: ProcessorAction,
    },
    /// Inspect individual products in the build graph (requires config)
    Product {
        #[command(subcommand)]
        action: ProductAction,
    },
    /// Count source lines of code (SLOC) by language (no config needed)
    Sloc {
        /// Show COCOMO effort/cost estimation
        #[arg(long)]
        cocomo: bool,
        /// Annual salary for COCOMO cost estimation (default: 56286)
        #[arg(long, default_value = "56286")]
        salary: u64,
    },
    /// Smart config manipulation commands
    Smart {
        #[command(subcommand)]
        action: SmartAction,
    },
    /// Show the status of each product (requires config)
    Status {
        /// Show source file counts by extension per processor
        #[arg(long)]
        breakdown: bool,
    },
    /// Create symlinks from source folders to target folders (requires config)
    SymlinkInstall,
    /// Search and query frontmatter tags from markdown files
    Tags {
        #[command(subcommand)]
        action: TagsAction,
    },
    /// Manage term checking and fixing in markdown files
    Terms {
        #[command(subcommand)]
        action: TermsAction,
    },
    /// Validate rsconstruct.toml configuration (no processors created)
    #[command(name = "toml")]
    Toml {
        #[command(subcommand)]
        action: TomlAction,
    },
    /// Manage external tool dependencies
    Tools {
        #[command(subcommand)]
        action: ToolsAction,
    },
    /// Print version information (no config needed)
    Version,
    /// Watch source files and auto-rebuild on changes (requires config)
    Watch {
        #[command(flatten)]
        shared: SharedBuildArgs,
    },
    /// Manage the web request cache
    #[command(name = "webcache")]
    WebCache {
        #[command(subcommand)]
        action: WebCacheAction,
    },
}

#[derive(Subcommand)]
pub enum SmartAction {
    /// Auto-detect relevant processors and add them to rsconstruct.toml (requires config)
    Auto,
    /// Disable a single processor in rsconstruct.toml (no config needed)
    Disable {
        /// Processor name
        name: String,
    },
    /// Disable all processors in rsconstruct.toml (no config needed)
    DisableAll,
    /// Enable a single processor in rsconstruct.toml (no config needed)
    Enable {
        /// Processor name
        name: String,
    },
    /// Enable all processors in rsconstruct.toml (no config needed)
    EnableAll,
    /// Enable only processors whose files are detected in the project (requires config)
    EnableDetected,
    /// Enable only processors whose files are detected and tools are installed (requires config)
    EnableIfAvailable,
    /// Disable all, then enable only detected processors (requires config)
    Minimal,
    /// Disable all, then enable only the listed processors (no config needed)
    Only {
        /// Processor names to enable
        #[arg(required = true)]
        names: Vec<String>,
    },
    /// Remove processors from rsconstruct.toml that don't match any files (requires config)
    RemoveNoFileProcessors,
    /// Remove all [processor.*] sections, returning to pure defaults (no config needed)
    Reset,
}

#[derive(Subcommand)]
pub enum InfoAction {
    /// Show source file counts by extension (requires config)
    Source,
}

#[derive(Subcommand)]
pub enum PhasesAction {
    /// List registered phase hooks (no config needed)
    Hooks {
        /// Filter to a single phase
        #[arg(value_enum)]
        phase: Option<crate::phases::Phase>,
    },
    /// List all phases with descriptions (no config needed)
    List,
}

#[derive(Subcommand)]
pub enum GraphAction {
    /// For each input file, show the products that consume it (forward lookup: source → outputs)
    LookupFwd {
        /// One or more file paths (relative to project root)
        #[arg(required = true)]
        files: Vec<String>,
    },
    /// For each output file, show the product that produced it (reverse lookup: output → sources)
    LookupRev {
        /// One or more file paths (relative to project root)
        #[arg(required = true)]
        files: Vec<String>,
    },
    /// Print the dependency graph to stdout (requires config)
    Show {
        /// Output format
        #[arg(short, long, value_enum, default_value = "svg")]
        format: GraphFormat,
    },
    /// Show graph statistics (requires config)
    Stats,
    /// List files on disk not referenced by any product in the graph (requires config)
    Unreferenced {
        /// File extensions to check, comma-separated (e.g. .svg,.png)
        #[arg(short, long, value_delimiter = ',', required = true)]
        extensions: Vec<String>,
        /// Delete the unreferenced files
        #[arg(long)]
        rm: bool,
    },
    /// Open the dependency graph in a viewer (requires config)
    View {
        /// Viewer to use
        #[arg(long, value_enum, default_value = "svg")]
        viewer: GraphViewer,
    },
}

#[derive(Subcommand)]
pub enum FixAction {
    /// List all fix-capable processors declared in this project (requires config)
    List,
    /// Run a fixer on source files (requires config)
    Run {
        /// Processor name (comma-separated for multiple)
        #[arg(value_delimiter = ',')]
        processors: Vec<String>,
    },
}

#[derive(Subcommand)]
pub enum CleanAction {
    /// Remove all build outputs and cache directories (requires config)
    All,
    /// Hard clean using git clean (requires config)
    Git,
    /// Remove build output files, preserves cache (requires config) [default]
    Outputs {
        /// Only clean outputs from these processors (comma-separated)
        #[arg(short, long, value_delimiter = ',')]
        processors: Vec<String>,
        /// Skip the post-clean sweep that removes directories left empty
        #[arg(long)]
        no_empty_dirs: bool,
    },
    /// Remove files not tracked by git and not known as build outputs (requires config)
    Unknown {
        /// Show what would be removed without actually deleting
        #[arg(long)]
        dry_run: bool,
        /// Include gitignored files as unknown (by default they are skipped)
        #[arg(long)]
        no_gitignore: bool,
    },
}

#[derive(Subcommand)]
pub enum CacheAction {
    /// Clear the entire cache (no config needed)
    Clear,
    /// List all cache entries and their status (requires config)
    List,
    /// Remove stale index entries not matching any current product (requires config)
    RemoveStale,
    /// Show cache size (requires config)
    Size,
    /// Show which cache entries are stale vs current (requires config)
    Stale,
    /// Show per-processor cache statistics (requires config)
    Stats,
    /// Remove unreferenced objects from cache (requires config)
    Trim,
}

#[derive(Subcommand)]
pub enum WebCacheAction {
    /// Clear the web cache (no config needed)
    Clear,
    /// List all cached entries (no config needed)
    List,
    /// Show web cache statistics (no config needed)
    Stats,
}

#[derive(Subcommand)]
pub enum ProductAction {
    /// Show every input, hash piece, and cache state for a single product
    Show {
        /// Output path of the product (e.g. "README.md") or its primary
        /// input. The lookup tries output paths first; if none own the path,
        /// falls back to products that have it as primary input.
        path: String,
    },
}

#[derive(Subcommand)]
pub enum ProcessorAction {
    /// Add a processor to rsconstruct.toml with must-fill fields pre-populated and comments
    Add {
        /// Processor name (pname) — the type name (e.g., ruff, pip, tera)
        #[arg(value_parser = crate::registries::processor_name_parser())]
        pname: String,
        /// Print the generated TOML snippet to stdout instead of writing to rsconstruct.toml
        #[arg(long)]
        dry_run: bool,
    },
    /// Show the current processor allowlist (requires config)
    Allowlist,
    /// Show resolved configuration for a processor instance by iname (requires config)
    Config {
        /// Instance name (iname) as declared in rsconstruct.toml (omit to show all)
        iname: Option<String>,
        /// Show only fields that differ from the default configuration
        #[arg(short, long)]
        diff: bool,
    },
    /// Show default configuration for a processor type by pname (no config needed)
    Defconfig {
        /// Processor name (pname) — the type name (e.g., ruff, pip, tera)
        #[arg(value_parser = crate::registries::processor_name_parser())]
        pname: String,
    },
    /// Remove a processor stanza from rsconstruct.toml entirely (requires config)
    Delete {
        /// Instance name (iname) as declared in rsconstruct.toml
        iname: String,
    },
    /// Set enabled = false on a processor stanza in rsconstruct.toml (requires config)
    Disable {
        /// Instance name (iname) as declared in rsconstruct.toml
        iname: String,
    },
    /// Set enabled = true on a processor stanza in rsconstruct.toml (requires config)
    Enable {
        /// Instance name (iname) as declared in rsconstruct.toml
        iname: String,
    },
    /// Show source and target files for each processor (requires config)
    Files {
        /// Instance name (iname) as declared in rsconstruct.toml (omit to show all)
        iname: Option<String>,
        /// Show processor headers (e.g., "[ruff] (42 products)")
        #[arg(long)]
        headers: bool,
    },
    /// Show inter-processor dependencies (requires config)
    Graph {
        /// Output format
        #[arg(short, long, value_enum, default_value = "text")]
        format: GraphFormat,
    },
    /// List all built-in processors with type and description (no config needed)
    List {
        /// Filter by processor type (checker, generator, creator, explicit)
        #[arg(long = "type", value_name = "TYPE")]
        processor_type: Option<String>,
    },
    /// Show names of all enabled processors (one per line, requires config)
    Names,
    /// Show the recommended processor for each file extension (no config needed)
    Recommend,
    /// Search processors by name, description, or keywords
    Search {
        /// Search term (case-insensitive, matches name, description, and keywords)
        query: String,
    },
    /// List all processor types with descriptions
    Types,
    /// Show which processors are enabled and detected (requires config)
    Used,
}

#[derive(Subcommand)]
pub enum TomlAction {
    /// Validate rsconstruct.toml — check for unknown fields, type errors, and missing required fields
    Check,
}

#[derive(Subcommand)]
pub enum FunctionsAction {
    /// List all built-in Tera template functions with documentation
    List,
}

#[derive(Subcommand)]
pub enum ToolsAction {
    /// Verify tool versions against .tools.versions lock file (uses config if available)
    Check,
    /// Show tool-to-processor dependency graph (uses config if available)
    Graph {
        /// Output format
        #[arg(short, long, value_enum, default_value = "dot")]
        format: GraphFormat,
        /// Open the graph in a browser instead of printing to stdout
        #[arg(long)]
        view: bool,
    },
    /// Install missing external tools (uses config if available)
    Install {
        /// Tool name to install (omit to install missing tools for enabled processors)
        name: Option<String>,
        /// Skip confirmation prompt
        #[arg(short, long)]
        yes: bool,
        /// Don't wrap apt/dnf/pacman calls with `eatmydata` (faster but
        /// safer; the wrap is on by default when eatmydata is installed)
        #[arg(long)]
        no_eatmydata: bool,
    },
    /// Install declared dependencies from the [dependencies] config section (uses config if available)
    InstallDeps {
        /// Skip confirmation prompt
        #[arg(short, long)]
        yes: bool,
        /// Don't wrap apt/dnf/pacman calls with `eatmydata` (faster but
        /// safer; the wrap is on by default when eatmydata is installed)
        #[arg(long)]
        no_eatmydata: bool,
    },
    /// List all tools known to rsconstruct, from the central registry (no config needed)
    List {
        /// Show all available installation methods for each tool
        #[arg(short = 'M', long)]
        methods: bool,
    },
    /// List external tools required by this project's processors (uses config if available)
    ListConfigured {
        /// Include tools from disabled processors too
        #[arg(short, long)]
        all: bool,
        /// Show all available installation methods for each tool
        #[arg(short = 'M', long)]
        methods: bool,
    },
    /// Lock tool versions to .tools.versions (uses config if available)
    Lock,
    /// Show tool availability statistics (uses config if available)
    Stats,
}

#[derive(Subcommand)]
pub enum AnalyzersAction {
    /// Add an analyzer to rsconstruct.toml with must-fill fields pre-populated and comments
    Add {
        /// Analyzer type name (pname) (e.g., cpp, icpp, python, markdown, tera)
        #[arg(value_parser = crate::registries::analyzer_name_parser())]
        pname: String,
        /// Print the generated TOML snippet to stdout instead of writing to rsconstruct.toml
        #[arg(long)]
        dry_run: bool,
    },
    /// Run dependency analysis without building (requires config)
    Build,
    /// Clear the dependency cache (requires config)
    Clean {
        /// Only clear entries tagged with this analyzer iname (e.g., "cpp", "cpp.kernel")
        #[arg(long)]
        analyzer: Option<String>,
    },
    /// Show resolved analyzer configuration by iname (requires config)
    Config {
        /// Analyzer instance name (iname) as declared in rsconstruct.toml; omit to show all
        iname: Option<String>,
    },
    /// Show default analyzer configuration by pname (no config needed)
    Defconfig {
        /// Analyzer type name (pname) (e.g., cpp, icpp, python); omit to show all
        #[arg(value_parser = crate::registries::analyzer_name_parser())]
        pname: Option<String>,
    },
    /// Remove an analyzer stanza from rsconstruct.toml entirely (requires config)
    Delete {
        /// Instance name (iname) as declared in rsconstruct.toml
        iname: String,
    },
    /// Set enabled = false on an analyzer stanza in rsconstruct.toml (requires config)
    Disable {
        /// Instance name (iname) as declared in rsconstruct.toml
        iname: String,
    },
    /// Set enabled = true on an analyzer stanza in rsconstruct.toml (requires config)
    Enable {
        /// Instance name (iname) as declared in rsconstruct.toml
        iname: String,
    },
    /// List all available dependency analyzers (no config needed)
    List,
    /// Show cached dependencies (requires config)
    Show {
        #[command(subcommand)]
        filter: AnalyzersShowFilter,
    },
    /// Show statistics about cached dependencies by analyzer (requires config)
    Stats,
    /// Show which dependency analyzers are enabled and detected (requires config)
    Used,
}

#[derive(Subcommand)]
pub enum AnalyzersShowFilter {
    /// Show dependencies for all source files
    All,
    /// Show dependencies for files handled by specific analyzers
    Analyzers {
        /// Analyzer names (e.g., "cpp", "python")
        #[arg(required = true)]
        analyzers: Vec<String>,
    },
    /// Show dependencies for specific files
    Files {
        /// Source files to show dependencies for
        #[arg(required = true)]
        files: Vec<String>,
        /// Also print the structured hash pieces each analyzer contributes
        /// to the product's cache key (e.g. resolved glob/git_count file
        /// sets, embedded shell commands). Recomputed live — reflects the
        /// current filesystem state, not the last build's state.
        #[arg(long)]
        hash_pieces: bool,
    },
}

#[derive(Subcommand)]
pub enum TermsAction {
    /// Auto-fix: add backticks to terms (requires config)
    Fix {
        /// Also remove backticks from non-terms
        #[arg(long, default_value_t = false)]
        remove_non_terms: bool,
    },
    /// Merge terms from another project's terms directory (requires config)
    Merge {
        /// Path to the other project's terms directory
        path: String,
    },
    /// Show term file and term count statistics (requires config)
    Stats,
}

#[derive(Subcommand)]
pub enum TagsAction {
    /// Run all tag validations without building (requires config)
    Check,
    /// Scan source files and add missing tags back to the tag collection (requires config)
    Collect,
    /// Show each tag with its file count, sorted by frequency (requires config)
    Count,
    /// Show percentage of files that have each tag category (requires config)
    Coverage,
    /// List files matching given tags, AND by default, --or for OR (requires config)
    Files {
        /// Tags: bare values (e.g. "docker") or key:value (e.g. "level:advanced")
        #[arg(required = true)]
        tags: Vec<String>,
        /// Use OR semantics (match files with any of the given tags)
        #[arg(long, short)]
        or: bool,
    },
    /// List all tags for a specific file (requires config)
    ForFile {
        /// Path to the file
        path: String,
    },
    /// Show the raw frontmatter for a specific file (requires config)
    Frontmatter {
        /// Path to the file
        path: String,
    },
    /// Search for tags containing a substring (requires config)
    Grep {
        /// Text to search for in tag names
        text: String,
        /// Case-insensitive search
        #[arg(short, long)]
        ignore_case: bool,
    },
    /// List all unique tags (requires config)
    List,
    /// Show a coverage matrix of tag categories per file (requires config)
    Matrix,
    /// Merge tags from another project's tags directory (requires config)
    Merge {
        /// Path to the other project's tags directory
        path: String,
    },
    /// Find markdown files with no tags at all (requires config)
    Orphans,
    /// Show statistics about the tags database (requires config)
    Stats,
    /// Suggest tags for a file based on similarity (requires config)
    Suggest {
        /// Path to the file
        path: String,
    },
    /// Show tags grouped by prefix/category (requires config)
    Tree,
    /// List tags in the allowlist (tags_dir) that are not used by any file (requires config)
    Unused {
        /// Exit with error if unused tags are found (useful for CI)
        #[arg(long)]
        strict: bool,
    },
    /// Validate tags against the allowlist without building (requires config)
    Validate,
}

/// CLI arguments shared between Build and Watch commands.
#[derive(Args, Clone)]
pub struct SharedBuildArgs {
    /// Number of parallel jobs (overrides config file)
    #[arg(short, long)]
    pub jobs: Option<usize>,

    /// Show per-product and total build timing information
    #[arg(long)]
    pub timings: bool,

    /// Continue building after errors, skipping dependents of failed products
    #[arg(short = 'k', long)]
    pub keep_going: bool,

    /// Suppress the build summary
    #[arg(long)]
    pub no_summary: bool,

    /// Batch size for batch-capable processors (0 = no limit, -1 = disable, omit to use config)
    #[arg(long, allow_negative_numbers = true)]
    pub batch_size: Option<i32>,

    /// Only run specific processors, by instance name (iname) as declared in rsconstruct.toml (comma-separated list)
    #[arg(short, long, value_delimiter = ',')]
    pub processors: Option<Vec<String>>,

    /// Exclude specific processors by instance name (comma-separated list).
    /// Mirrors `-p` and accepts the same shortcuts (e.g. `@checkers`, `@python3`).
    /// When combined with `-p`, excludes are subtracted from the included set;
    /// a processor appearing in both is an error.
    #[arg(short = 'x', long = "exclude-processors", value_delimiter = ',')]
    pub exclude_processors: Option<Vec<String>>,

    /// Automatically add misspelled words to words files instead of failing (zspell + aspell)
    #[arg(long)]
    pub auto_add_words: bool,

    /// Show why each product is skipped, restored, or rebuilt
    #[arg(long)]
    pub explain: bool,

    /// Retry failed products up to N times to detect flakiness
    #[arg(long, value_name = "N", default_value = "0")]
    pub retry: usize,

    /// Disable mtime pre-check (always compute full checksums)
    #[arg(long)]
    pub no_mtime: bool,

    /// Only build products matching these file patterns (glob syntax, repeatable)
    #[arg(short, long = "target")]
    pub targets: Option<Vec<String>>,

    /// Only build products whose inputs are under these directories (repeatable)
    #[arg(short, long = "dir")]
    pub dirs: Option<Vec<String>>,

    /// Write a Chrome trace JSON file for build visualization (open in chrome://tracing or Perfetto)
    #[arg(long, value_name = "FILE")]
    pub trace: Option<String>,

    /// Show all config changes between runs (not just output-affecting fields)
    #[arg(long)]
    pub show_all_config_changes: bool,

    /// Override a per-instance config field (repeatable). Format: `<iname>.<field>=<value>`.
    /// Value is parsed as TOML (e.g. `marp.max_jobs=2`, `marp.batch=true`, `marp.args=["--html"]`);
    /// strings without quoting are accepted (e.g. `marp.command=marp`).
    #[arg(long, value_name = "INAME.FIELD=VALUE")]
    pub iset: Vec<String>,

    /// Override a config field on every instance of a processor type (repeatable).
    /// Format: `<pname>.<field>=<value>`. Same value parsing as `--iset`.
    /// Errors if no instances of that pname exist.
    #[arg(long, value_name = "PNAME.FIELD=VALUE")]
    pub pset: Vec<String>,
}

impl SharedBuildArgs {
    /// Convert to BuildOptions with the given overrides for build-only fields.
    pub fn to_build_options(&self, cli: &Cli, force: bool, stop_after: BuildPhase) -> BuildOptions {
        // Merge --dir values into targets as glob patterns
        let targets = match (&self.targets, &self.dirs) {
            (None, None) => None,
            (Some(t), None) => Some(t.clone()),
            (None, Some(d)) => Some(d.iter().map(|dir| format!("{dir}/**")).collect()),
            (Some(t), Some(d)) => {
                let mut merged = t.clone();
                merged.extend(d.iter().map(|dir| format!("{dir}/**")));
                Some(merged)
            }
        };
        BuildOptions {
            force,
            verbose: cli.verbose,
            display_opts: cli.display_options(),
            jobs: self.jobs,
            timings: self.timings,
            keep_going: self.keep_going,
            summary: !self.no_summary,
            batch_size: self.batch_size.map(|n| if n < 0 { None } else { Some(n as usize) }),
            stop_after,
            processor_filter: self.processors.clone(),
            exclude_filter: self.exclude_processors.clone(),
            auto_add_words: self.auto_add_words,
            explain: self.explain,
            no_mtime: self.no_mtime,
            retry: self.retry,
            targets,
            trace: self.trace.clone(),
            show_all_config_changes: self.show_all_config_changes,
            iset: self.iset.clone(),
            pset: self.pset.clone(),
        }
    }
}

/// Options shared by build and watch commands.
#[derive(Clone)]
pub struct BuildOptions {
    pub force: bool,
    pub verbose: bool,
    pub display_opts: DisplayOptions,
    pub jobs: Option<usize>,
    pub timings: bool,
    pub keep_going: bool,
    pub summary: bool,
    pub batch_size: Option<Option<usize>>,
    pub stop_after: BuildPhase,
    pub processor_filter: Option<Vec<String>>,
    pub exclude_filter: Option<Vec<String>>,
    pub auto_add_words: bool,
    pub explain: bool,
    pub no_mtime: bool,
    pub retry: usize,
    pub targets: Option<Vec<String>>,
    pub trace: Option<String>,
    pub show_all_config_changes: bool,
    pub iset: Vec<String>,
    pub pset: Vec<String>,
}

/// Parse a shell name string into a Shell enum
pub fn parse_shell(name: &str) -> Option<Shell> {
    <Shell as FromStr>::from_str(name).ok()
}

/// Recursively set `hide_short_help = true` on all arguments in a command and its subcommands.
fn hide_all_flags(cmd: clap::Command) -> clap::Command {
    let cmd = cmd.mut_args(|arg| {
        if arg.get_long().is_some() || arg.get_short().is_some() {
            arg.hide_short_help(true)
        } else {
            arg
        }
    });
    cmd.mut_subcommands(hide_all_flags)
}

/// Parse CLI arguments with all flags hidden from short help (`-h`).
/// Use `--help` to see all flags.
pub fn parse_cli() -> Cli {
    let cmd = hide_all_flags(Cli::command());
    let matches = cmd.get_matches();
    Cli::from_arg_matches(&matches).expect("failed to parse CLI arguments")
}

/// Generate shell completions and print to stdout.
/// Post-processes the generated script to inject processor name completions
/// for `processors config`, `processors defconfig`, and `processors files`.
pub fn print_completions(shell: Shell) -> Result<()> {
    let script = generate_completion_script(shell)?;
    print!("{script}");
    Ok(())
}

/// Generate the completion script for a shell as a string.
/// Separated from `print_completions` so tests can round-trip the real
/// clap_complete output through the bash injection.
fn generate_completion_script(shell: Shell) -> Result<String> {
    let mut cmd = Cli::command();
    let mut buf = Vec::new();
    generate(shell, &mut cmd, "rsconstruct", &mut buf);
    let script = String::from_utf8(buf).expect("completion script should be UTF-8");

    match shell {
        Shell::Bash => inject_bash_processor_completions(&script),
        _ => Ok(script),
    }
}

/// Inject processor/analyzer completion into a bash completion script.
///
/// - `processors config`, `processors files`, `processors delete`, `processors disable`,
///   `processors enable` complete with **processor instance names** (inames) from
///   `rsconstruct.toml`.
/// - `analyzers delete`, `analyzers disable`, `analyzers enable` complete with **analyzer
///   instance names** (inames) from `rsconstruct.toml`.
/// - `--processors` / `-p` flags in `build`/`watch` complete with **instance
///   names** (inames) from `rsconstruct.toml` — you can only build a processor
///   that is declared in the project.
/// - `processors defconfig` is handled automatically by clap via `#[arg(value_parser = ...)]`.
///
/// Errors if any injection point is not found in the script — clap-complete
/// controls the script format, so a miss means its output changed and the
/// target labels/needles below must be updated. The unit test
/// `bash_iname_injection_matches_clap_complete_output` round-trips the real
/// generated script, so a format change fails `cargo test` before it can
/// reach a user.
fn inject_bash_processor_completions(script: &str) -> Result<String> {
    // Bash helpers: extract instance names from rsconstruct.toml.
    let helper = r#"
_rsconstruct_inames() {
    local toml="rsconstruct.toml"
    [[ -f "$toml" ]] || return
    # Match [processor.NAME] -> NAME; [processor.NAME.SUB] -> NAME.SUB
    grep -E '^\[processor\.' "$toml" | sed -E 's/^\[processor\.([^]]+)\].*/\1/'
}
_rsconstruct_analyzer_inames() {
    local toml="rsconstruct.toml"
    [[ -f "$toml" ]] || return
    # Match [analyzer.NAME] -> NAME; [analyzer.NAME.SUB] -> NAME.SUB
    grep -E '^\[analyzer\.' "$toml" | sed -E 's/^\[analyzer\.([^]]+)\].*/\1/'
}
_rsconstruct_fixer_inames() {
    # Filter inames to only processors with fix capability.
    # Types with built-in fix: ruff, black, prettier, eslint, stylelint, standard, taplo, rumdl, markdownlint.
    # For script processors: check if fix_command is set in the toml section.
    local toml="rsconstruct.toml"
    [[ -f "$toml" ]] || return
    local fixers="ruff|black|prettier|eslint|stylelint|standard|taplo|rumdl|markdownlint"
    for iname in $(_rsconstruct_inames); do
        local pname="${iname%%.*}"
        if echo "$pname" | grep -qE "^($fixers)$"; then
            echo "$iname"
        elif [ "$pname" = "script" ]; then
            # Check if fix_command is set in this script section
            if sed -n "/^\[processor\.$iname\]/,/^\[/p" "$toml" | grep -q 'fix_command'; then
                echo "$iname"
            fi
        fi
    done
}
"#;

    // Replace instance-name targets to call _rsconstruct_inames at tab time.
    // For each target section, replace the early-return COMPREPLY with a call to our helper.
    let iname_targets = [
        ("rsconstruct__subcmd__processors__subcmd__files)", 3),
        ("rsconstruct__subcmd__processors__subcmd__config)", 3),
        ("rsconstruct__subcmd__processors__subcmd__delete)", 3),
        ("rsconstruct__subcmd__processors__subcmd__disable)", 3),
        ("rsconstruct__subcmd__processors__subcmd__enable)", 3),
    ];

    // Failed injections must be reported: silently shipping completions
    // without iname support would hide a clap-complete format change.
    let mut missed: Vec<String> = Vec::new();

    let mut result = script.to_string();
    for (target, _) in &iname_targets {
        if let Some(section_start) = result.find(target) {
            let after_start = section_start + target.len();
            let section_len = result[after_start..]
                .find("\n        rsconstruct__")
                .unwrap_or(result.len() - after_start);
            let section_end = after_start + section_len;

            // Replace the early COMPREPLY line with one that calls our helper.
            // The generated code looks like:
            //   if [[ ${cur} == -* || ${COMP_CWORD} -eq N ]] ; then
            //       COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            //       return 0
            //   fi
            // We inject a check: if it's a positional (not a flag), use our helper.
            let section_slice = &result[section_start..section_end];
            let needle = "COMPREPLY=( $(compgen -W \"${opts}\" -- \"${cur}\") )";
            if let Some(rel_pos) = section_slice.find(needle) {
                let abs_pos = section_start + rel_pos;
                let replacement = "if [[ ${cur} != -* ]] ; then\n                    COMPREPLY=( $(compgen -W \"$(_rsconstruct_inames)\" -- \"${cur}\") )\n                else\n                    COMPREPLY=( $(compgen -W \"${opts}\" -- \"${cur}\") )\n                fi";
                result.replace_range(abs_pos..abs_pos + needle.len(), replacement);
            } else {
                missed.push(format!("COMPREPLY line in section {target}"));
            }
        } else {
            missed.push(format!("section {target}"));
        }
    }

    // Inject analyzer iname completion for delete/disable/enable.
    let analyzer_iname_targets = [
        "rsconstruct__subcmd__analyzers__subcmd__delete)",
        "rsconstruct__subcmd__analyzers__subcmd__disable)",
        "rsconstruct__subcmd__analyzers__subcmd__enable)",
    ];
    for target in &analyzer_iname_targets {
        if let Some(section_start) = result.find(target) {
            let after_start = section_start + target.len();
            let section_len = result[after_start..]
                .find("\n        rsconstruct__")
                .unwrap_or(result.len() - after_start);
            let section_end = after_start + section_len;
            let section_slice = &result[section_start..section_end];
            let needle = "COMPREPLY=( $(compgen -W \"${opts}\" -- \"${cur}\") )";
            if let Some(rel_pos) = section_slice.find(needle) {
                let abs_pos = section_start + rel_pos;
                let replacement = "if [[ ${cur} != -* ]] ; then\n                    COMPREPLY=( $(compgen -W \"$(_rsconstruct_analyzer_inames)\" -- \"${cur}\") )\n                else\n                    COMPREPLY=( $(compgen -W \"${opts}\" -- \"${cur}\") )\n                fi";
                result.replace_range(abs_pos..abs_pos + needle.len(), replacement);
            } else {
                missed.push(format!("COMPREPLY line in section {target}"));
            }
        } else {
            missed.push(format!("section {target}"));
        }
    }

    // Inject completion for --processors/-p in build/watch: inames only,
    // read from rsconstruct.toml at tab time via _rsconstruct_inames. You can
    // only build a processor that is declared in the project.
    let old_processors = "                --processors)\n                    COMPREPLY=($(compgen -f \"${cur}\"))";
    let new_processors = "                --processors)\n                    COMPREPLY=($(compgen -W \"$(_rsconstruct_inames)\" -- \"${cur}\"))".to_string();
    let old_p = "                -p)\n                    COMPREPLY=($(compgen -f \"${cur}\"))";
    let new_p = "                -p)\n                    COMPREPLY=($(compgen -W \"$(_rsconstruct_inames)\" -- \"${cur}\"))".to_string();
    let old_exclude = "                --exclude-processors)\n                    COMPREPLY=($(compgen -f \"${cur}\"))";
    let new_exclude = "                --exclude-processors)\n                    COMPREPLY=($(compgen -W \"$(_rsconstruct_inames)\" -- \"${cur}\"))".to_string();
    let old_x = "                -x)\n                    COMPREPLY=($(compgen -f \"${cur}\"))";
    let new_x = "                -x)\n                    COMPREPLY=($(compgen -W \"$(_rsconstruct_inames)\" -- \"${cur}\"))".to_string();

    for (label, old) in [("--processors", old_processors), ("-p", old_p), ("--exclude-processors", old_exclude), ("-x", old_x)] {
        if !result.contains(old) {
            missed.push(format!("{label} flag completion"));
        }
    }
    let mut result = result
        .replace(old_processors, &new_processors)
        .replace(old_p, &new_p)
        .replace(old_exclude, &new_exclude)
        .replace(old_x, &new_x);

    // Inject completion for `fix run <processor>`: only fixer inames.
    // The `fix run` subcommand takes positional processor names — inject
    // our fixer helper so tab-completing shows only fix-capable processors
    // declared in the project.
    let iname_targets_fix = [
        "rsconstruct__subcmd__fix__subcmd__run)",
    ];
    for target in &iname_targets_fix {
        if let Some(section_start) = result.find(target) {
            let after_start = section_start + target.len();
            let section_len = result[after_start..]
                .find("\n        rsconstruct__")
                .unwrap_or(result.len() - after_start);
            let section_end = after_start + section_len;
            let section_slice = &result[section_start..section_end];
            let needle = "COMPREPLY=( $(compgen -W \"${opts}\" -- \"${cur}\") )";
            if let Some(rel_pos) = section_slice.find(needle) {
                let abs_pos = section_start + rel_pos;
                let replacement = "if [[ ${cur} != -* ]] ; then\n                    COMPREPLY=( $(compgen -W \"$(_rsconstruct_fixer_inames)\" -- \"${cur}\") )\n                else\n                    COMPREPLY=( $(compgen -W \"${opts}\" -- \"${cur}\") )\n                fi";
                result.replace_range(abs_pos..abs_pos + needle.len(), replacement);
            } else {
                missed.push(format!("COMPREPLY line in section {target}"));
            }
        } else {
            missed.push(format!("section {target}"));
        }
    }

    if !missed.is_empty() {
        bail!(
            "bash completion iname injection failed for: {} — clap-complete output format changed; update the labels/needles in inject_bash_processor_completions",
            missed.join(", ")
        );
    }

    // Prepend the helper function before the completion function is defined.
    Ok(format!("{helper}{result}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bash iname injection locates its patch points in clap_complete's
    /// generated script by exact string matching. clap-complete owns that
    /// format and has changed it before (case labels went from
    /// `rsconstruct__processors__files` to
    /// `rsconstruct__subcmd__processors__subcmd__files`). Round-trip the real
    /// generated script through the injection so any future format change
    /// fails here instead of surfacing when a user sources their completions.
    #[test]
    fn bash_iname_injection_matches_clap_complete_output() {
        let script = generate_completion_script(Shell::Bash)
            .expect("bash iname injection failed — clap-complete output format changed");

        for helper_call in [
            "compgen -W \"$(_rsconstruct_inames)\"",
            "compgen -W \"$(_rsconstruct_analyzer_inames)\"",
            "compgen -W \"$(_rsconstruct_fixer_inames)\"",
        ] {
            assert!(
                script.contains(helper_call),
                "injected helper call missing from bash completion script: {helper_call}"
            );
        }
    }

    /// Non-bash shells must still generate without error.
    #[test]
    fn zsh_and_fish_completion_scripts_generate() {
        for shell in [Shell::Zsh, Shell::Fish] {
            let script = generate_completion_script(shell)
                .unwrap_or_else(|e| panic!("completion generation failed for {shell}: {e}"));
            assert!(!script.is_empty(), "empty completion script for {shell}");
        }
    }
}
