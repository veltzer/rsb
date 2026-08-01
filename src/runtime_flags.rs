//! Centralized runtime flags set once at startup and read throughout execution.
//!
//! Replaces individual `AtomicBool` statics scattered across modules with a single
//! struct stored in a `OnceLock`. All flags are immutable after initialization.

use std::sync::OnceLock;

/// Runtime flags set once at startup from CLI arguments.
#[derive(Debug)]
pub struct RuntimeFlags {
    /// Print each child process command before execution (--show-child-processes)
    pub show_child_processes: bool,
    /// Show tool output even on success (--show-output)
    pub show_output: bool,
    /// Print phase messages during graph building (--phases)
    pub phases_debug: bool,
    /// Print graph size at each major build stage (--graph-stats)
    pub graph_stats: bool,
    /// Output JSON instead of human-readable text (--json)
    pub json_mode: bool,
    /// Suppress all output except errors (--quiet)
    pub quiet: bool,
    /// Whether to emit ANSI color escape sequences.
    /// Resolved from --color (auto/always/never) and the NO_COLOR env var.
    pub color_enabled: bool,
}

static FLAGS: OnceLock<RuntimeFlags> = OnceLock::new();

/// Initialize runtime flags. Must be called exactly once from main before any reads.
pub fn init(flags: RuntimeFlags) {
    FLAGS.set(flags).expect("runtime flags already initialized");
}

/// Initialize with defaults if not already initialized. Test-only: unit
/// tests share one process, so whichever test runs first would win a plain
/// `init()` and the rest would panic on the double-set.
#[cfg(test)]
pub fn init_for_test() {
    let _ = FLAGS.set(RuntimeFlags {
        show_child_processes: false,
        show_output: false,
        phases_debug: false,
        graph_stats: false,
        json_mode: false,
        quiet: true,
        color_enabled: false,
    });
}

/// Get the runtime flags. Panics if called before `init()`.
fn get() -> &'static RuntimeFlags {
    FLAGS.get().expect("runtime flags not initialized")
}

pub fn show_child_processes() -> bool {
    get().show_child_processes
}

pub fn show_output() -> bool {
    get().show_output
}

pub fn phases_debug() -> bool {
    get().phases_debug
}

pub fn graph_stats() -> bool {
    get().graph_stats
}

pub fn json_mode() -> bool {
    get().json_mode
}

pub fn quiet() -> bool {
    get().quiet
}

pub fn color_enabled() -> bool {
    // If flags aren't initialized yet, fall back to "no color". This can happen
    // during very early startup (e.g., CLI parse errors from clap).
    FLAGS.get().map(|f| f.color_enabled).unwrap_or(false)
}

/// Non-panicking read of `quiet`, safe to call before `init()` — used by the
/// final exit-status line in `main()`, which can run even when CLI parsing
/// failed before flags were set up.
pub fn quiet_or_default() -> bool {
    FLAGS.get().map(|f| f.quiet).unwrap_or(false)
}

/// Non-panicking read of `json_mode`. Same rationale as `quiet_or_default`.
pub fn json_mode_or_default() -> bool {
    FLAGS.get().map(|f| f.json_mode).unwrap_or(false)
}
