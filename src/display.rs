//! How a product is rendered in build messages.
//!
//! These live here rather than in `cli` because `graph.rs` — the core data
//! model — needs them, and a core module importing `crate::cli` inverts the
//! layering: it makes the data model depend on the command-line front end.
//! They still derive `ValueEnum` so clap can parse them as flag values; that
//! derive does not require living in the CLI module.

use clap::ValueEnum;

/// What to show for output files in build messages
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
pub enum OutputDisplay {
    /// Don't show output files
    #[default]
    None,
    /// Show only the filename (e.g., "main.elf")
    Basename,
    /// Show full relative path (e.g., "`out/cc_single_file/main.elf`")
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
