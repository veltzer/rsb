//! Build statistics and timing summaries.
//!
//! Lives at the crate root because nothing here touches the `Processor`
//! trait: these are the counters the executor accumulates and the summary
//! lines it prints at the end of a build.

use std::time::Duration;

use crate::color;

/// Timing for a single product execution
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductTiming {
    pub display: String,
    pub processor: String,
    pub duration: Duration,
    /// Offset from the build start time (for trace output)
    pub start_offset: Option<Duration>,
}

/// Statistics from processing a category of items
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ProcessStats {
    pub processed: usize,
    pub failed: usize,
    pub flaky: usize,
    pub skipped: usize,
    pub restored: usize,
    pub files_created: usize,
    pub files_restored: usize,
    pub duration: Duration,
    pub product_timings: Vec<ProductTiming>,
}

impl ProcessStats {
    pub const fn total(&self) -> usize {
        self.processed + self.failed + self.skipped + self.restored
    }
}

/// Aggregated statistics from all processors
#[derive(Default)]
pub struct BuildStats {
    pub categories: Vec<ProcessStats>,
    pub total_duration: Duration,
    pub failed_count: usize,
    pub failed_messages: Vec<String>,
    pub phase_timings: Vec<(String, Duration)>,
}

impl BuildStats {
    pub fn add(&mut self, stats: ProcessStats) {
        if stats.total() > 0 {
            self.categories.push(stats);
        }
    }

    pub fn total_processed(&self) -> usize {
        self.categories.iter().map(|s| s.processed).sum()
    }

    pub fn total_skipped(&self) -> usize {
        self.categories.iter().map(|s| s.skipped).sum()
    }

    pub fn total_restored(&self) -> usize {
        self.categories.iter().map(|s| s.restored).sum()
    }

    pub fn total_files_created(&self) -> usize {
        self.categories.iter().map(|s| s.files_created).sum()
    }

    pub fn total_files_restored(&self) -> usize {
        self.categories.iter().map(|s| s.files_restored).sum()
    }

    pub fn total_flaky(&self) -> usize {
        self.categories.iter().map(|s| s.flaky).sum()
    }

    pub fn print_summary(&self, summary: bool, timings: bool) {
        // Don't print human-readable summary in JSON or quiet mode
        if crate::json_output::is_json_mode() || crate::runtime_flags::quiet() {
            return;
        }

        if !summary && !timings {
            return;
        }

        if self.categories.is_empty() && self.failed_count == 0 {
            if summary {
                println!("{}", color::dim("Nothing to build."));
            }
            return;
        }

        if summary {
            let total_processed = self.total_processed();
            let total_restored = self.total_restored();
            let total_failed = self.failed_count;
            let total_skipped = self.total_skipped();
            let total_files_created = self.total_files_created();
            let total_files_restored = self.total_files_restored();

            let total_flaky = self.total_flaky();
            // Always show every category, including zero counts, so the line
            // shape is identical across builds and easy to scan/grep. Work
            // done (built, restored, failed) leads the line; idle counts
            // (unchanged, flaky) go in parentheses.
            let built_part = if total_files_created > 0 {
                format!("{total_processed} built ({total_files_created} files created)")
            } else {
                format!("{total_processed} built")
            };
            let restored_part = if total_files_restored > 0 {
                format!("{total_restored} restored ({total_files_restored} files)")
            } else {
                format!("{total_restored} restored")
            };
            let lead = format!(
                "{built_part}, {restored_part}, {total_failed} failed",
            );
            let aside = format!("{total_skipped} unchanged, {total_flaky} flaky");

            // Emitted without color: the final "Exited with ..." line printed
            // by main() is the one coloured green/red so there's a single
            // signal of overall success/failure.
            println!("[build] summary: {lead} ({aside})");
        }

        if self.failed_count > 0 {
            println!("{}", color::red(&format!("Build finished with {} error(s):", self.failed_count)));
            for msg in &self.failed_messages {
                println!("{} {}", color::red("*"), msg);
            }
        }

        if timings {
            println!();
            println!("{}", color::bold("Timing:"));

            // Phase timings
            if !self.phase_timings.is_empty() {
                let rows: Vec<Vec<String>> = self.phase_timings.iter()
                    .map(|(name, dur)| vec![name.clone(), format!("{:.3}s", dur.as_secs_f64())])
                    .collect();
                crate::tables::print_table(&["Phase", "Duration"], &rows);
            }

            // Per-product timings
            for cat in &self.categories {
                for pt in &cat.product_timings {
                    println!("[{}] {} {}", pt.processor, pt.display,
                        color::dim(&format!("({:.3}s)", pt.duration.as_secs_f64())));
                }
            }

            let total: f64 = self.phase_timings.iter().map(|(_, d)| d.as_secs_f64()).sum();
            println!("{}", color::bold(&format!("Total: {total:.3}s")));
        }
    }
}
