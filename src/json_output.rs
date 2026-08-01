//! JSON output mode for machine-readable build events.
//!
//! When enabled, rsconstruct outputs JSON Lines (one JSON object per line) instead of
//! human-readable text. This is useful for CI integration, build dashboards,
//! and IDE integration.

use serde::Serialize;

use crate::errors;
use std::io::{self, Write};
use std::time::Duration;

/// Check if JSON output mode is enabled.
pub fn is_json_mode() -> bool {
    crate::runtime_flags::json_mode()
}

/// Whether human-readable text may be written to stdout.
///
/// False under `--json` (stdout carries only JSON events, so any prose
/// would corrupt the stream) and under `--quiet`. Every human-facing
/// `println!` on the build path must be gated on this — checking only
/// `quiet()` is the mistake that let progress lines leak into `--json`
/// output.
/// Uses the non-panicking flag accessors so it is safe to call from library
/// code exercised by unit tests, which never run `runtime_flags::init`.
pub fn human_output_enabled() -> bool {
    !crate::runtime_flags::json_mode_or_default() && !crate::runtime_flags::quiet_or_default()
}

/// Status of a completed product in a build.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProductStatus {
    Success,
    Failed,
    Skipped,
    Restored,
}

/// Build event types for JSON output.
#[derive(Debug, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum BuildEvent {
    /// Build is starting
    BuildStart {
        /// Total number of products to process
        total_products: usize,
    },

    /// A product is about to be processed
    ProductStart {
        /// Product identifier
        product: String,
        /// Processor name
        processor: String,
    },

    /// A product completed successfully
    ProductComplete {
        /// Product identifier
        product: String,
        /// Processor name
        processor: String,
        /// Build status
        status: ProductStatus,
        /// Duration in milliseconds (if executed)
        #[serde(skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u64>,
        /// Error message (if failed)
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },

    /// Build completed
    BuildSummary {
        /// Total products
        total: usize,
        /// Successfully processed
        success: usize,
        /// Failed
        failed: usize,
        /// Skipped (unchanged)
        skipped: usize,
        /// Restored from cache
        restored: usize,
        /// Total duration in milliseconds
        duration_ms: u64,
        /// List of error messages
        #[serde(skip_serializing_if = "Vec::is_empty")]
        errors: Vec<String>,
    },

}

/// Processor file entry for `rsconstruct processors files --json`.
#[derive(Debug, Serialize)]
pub struct ProcessorFileEntry {
    pub processor: String,
    pub processor_type: String,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
}

/// Entry for `rsconstruct processors list --json`.
#[derive(Debug, Serialize)]
pub struct ProcessorListEntry {
    pub name: String,
    pub processor_type: String,
    pub enabled: bool,
    pub detected: bool,
    pub batch: bool,
    pub native: bool,
    pub fix: bool,
    pub description: String,
}

/// A single install method for a tool.
#[derive(Debug, Serialize)]
pub struct ToolInstallMethodEntry {
    pub method: String,
    pub command: String,
}

/// Entry for `rsconstruct tools list --json`.
#[derive(Debug, Serialize)]
pub struct ToolListEntry {
    pub tool: String,
    pub installed: bool,
    pub runtime: String,
    pub processors: Vec<String>,
    pub install_methods: Vec<ToolInstallMethodEntry>,
}

/// Output for `rsconstruct tools stats --json`.
#[derive(Debug, Serialize)]
pub struct ToolStatsOutput {
    pub tools: Vec<ToolStat>,
    pub runtimes: Vec<RuntimeStat>,
    pub summary: StatsSummary,
}

/// Per-tool availability info for `rsconstruct tools stats --json`.
#[derive(Debug, Serialize)]
pub struct ToolStat {
    pub name: String,
    pub installed: bool,
    pub runtime: String,
    pub processors: Vec<String>,
    pub install_command: Option<String>,
}

/// Per-runtime summary for `rsconstruct tools stats --json`.
#[derive(Debug, Serialize)]
pub struct RuntimeStat {
    pub runtime: String,
    pub total: usize,
    pub installed: usize,
    pub missing: usize,
}

/// Overall summary for `rsconstruct tools stats --json`.
#[derive(Debug, Serialize)]
pub struct StatsSummary {
    pub total_tools: usize,
    pub installed: usize,
    pub missing: usize,
}

/// Per-language SLOC entry for JSON output.
#[derive(Debug, Serialize)]
pub struct SlocLanguageEntry {
    pub language: String,
    pub files: usize,
    pub blank: usize,
    pub comment: usize,
    pub code: usize,
}

/// SLOC totals for JSON output.
#[derive(Debug, Serialize)]
pub struct SlocTotals {
    pub files: usize,
    pub blank: usize,
    pub comment: usize,
    pub code: usize,
}

/// COCOMO estimation for JSON output.
#[derive(Debug, Serialize)]
pub struct SlocCocomoEstimate {
    pub effort_person_months: f64,
    pub schedule_months: f64,
    pub people: f64,
    pub cost: f64,
    pub salary: u64,
}

/// Top-level SLOC output for JSON mode.
#[derive(Debug, Serialize)]
pub struct SlocOutput {
    pub languages: Vec<SlocLanguageEntry>,
    pub total: SlocTotals,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cocomo: Option<SlocCocomoEstimate>,
}

/// Emit a JSON event to stdout.
pub fn emit(event: &BuildEvent) {
    if !is_json_mode() {
        return;
    }

    let json = serde_json::to_string(event).expect(errors::JSON_SERIALIZE);
    let mut stdout = io::stdout().lock();
    // Intentionally discard write errors: stdout may be a broken pipe (SIGPIPE)
    // when the consumer closes early, which is not an error we can recover from.
    let _ = writeln!(stdout, "{json}");
}

/// Emit a build start event.
pub fn emit_build_start(total_products: usize) {
    emit(&BuildEvent::BuildStart { total_products });
}

/// Emit a product start event.
pub fn emit_product_start(product: &str, processor: &str) {
    emit(&BuildEvent::ProductStart {
        product: product.to_string(),
        processor: processor.to_string(),
    });
}

/// Emit a product complete event.
pub fn emit_product_complete(
    product: &str,
    processor: &str,
    status: ProductStatus,
    duration: Option<Duration>,
    error: Option<&str>,
) {
    emit(&BuildEvent::ProductComplete {
        product: product.to_string(),
        processor: processor.to_string(),
        status,
        duration_ms: duration.map(|d| d.as_millis() as u64),
        error: error.map(std::string::ToString::to_string),
    });
}

/// Emit a build summary event.
pub fn emit_build_summary(
    total: usize,
    success: usize,
    failed: usize,
    skipped: usize,
    restored: usize,
    duration: Duration,
    errors: &[String],
) {
    emit(&BuildEvent::BuildSummary {
        total,
        success,
        failed,
        skipped,
        restored,
        duration_ms: duration.as_millis() as u64,
        errors: errors.to_vec(),
    });
}

