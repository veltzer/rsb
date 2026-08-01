//! The single sink for human-readable output.
//!
//! Before this module, `--json` purity was maintained by remembering to
//! guard each of ~445 `println!` sites by hand. The guard was present at 12
//! of them. That is not a contract, it is a habit — and it had already
//! failed: plain `--json` leaked build-phase lines, and `--color=always`
//! put raw ANSI escapes on stdout.
//!
//! Everything human-facing on the build path goes through [`info`] /
//! [`detail`] / [`warn`] / [`error`], which consult the same predicate in
//! one place. Machine-readable output (`--json` events, and the JSON bodies
//! that command handlers print) goes to stdout directly and is never
//! suppressed — it *is* the output in that mode.
//!
//! ## Which stream
//!
//! - [`info`] and [`detail`] write to **stdout**: they are the program's
//!   answer to what the user asked.
//! - [`warn`] and [`error`] write to **stderr**: they must remain visible
//!   when stdout is being consumed as data, and must never interleave into
//!   a JSON stream.
//!
//! Command handlers that print a JSON document under `--json` still branch
//! on [`crate::json_output::is_json_mode`] and call `println!` for the
//! document itself. That is correct — the document is the machine output.

use std::io::Write;

/// Whether human-readable prose may be written to stdout right now.
///
/// False under `--json` (stdout carries only machine output) and under
/// `--quiet`. Re-exported from `json_output` so callers have one import.
pub fn enabled() -> bool {
    crate::json_output::human_output_enabled()
}

/// Print a line of human-readable output to stdout, unless suppressed.
///
/// This is the default for anything the user is meant to read.
pub fn info(msg: &str) {
    if !enabled() {
        return;
    }
    let mut out = std::io::stdout().lock();
    // Write errors are discarded deliberately: stdout may be a broken pipe
    // when the consumer closes early (`rsconstruct ... | head`), which is
    // not something we can or should recover from.
    let _ = writeln!(out, "{msg}");
}

/// Print a line of secondary detail — only shown when the caller is already
/// in a verbose mode. The verbosity decision belongs to the caller; this
/// exists so that intent is visible at the call site.
pub fn detail(verbose: bool, msg: &str) {
    if verbose {
        info(msg);
    }
}

/// Print a warning to stderr.
///
/// Not gated on [`enabled`]: a warning that vanishes under `--json` or
/// `--quiet` is a warning nobody acts on. stderr keeps it out of the
/// machine-readable stream.
pub fn warn(msg: &str) {
    let mut err = std::io::stderr().lock();
    let _ = writeln!(err, "{} {}", crate::color::yellow("Warning:"), msg);
}

/// Print an error to stderr. Never suppressed, for the same reason as
/// [`warn`].
pub fn error(msg: &str) {
    let mut err = std::io::stderr().lock();
    let _ = writeln!(err, "{} {}", crate::color::red("Error:"), msg);
}

/// Print a raw line to stderr with no prefix, unless suppressed by
/// `--quiet`. For diagnostic streams that are already formatted (phase
/// traces, `[exec]` lines) and would be noise with a `Warning:` prefix.
///
/// Not gated on JSON mode: stderr is not the JSON stream, so diagnostics
/// there are harmless to a consumer parsing stdout.
pub fn diagnostic(msg: &str) {
    if crate::runtime_flags::quiet_or_default() {
        return;
    }
    let mut err = std::io::stderr().lock();
    let _ = writeln!(err, "{msg}");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The predicate is the whole point of the module: it must agree with
    /// `json_output::human_output_enabled` rather than drift from it.
    #[test]
    fn enabled_tracks_the_json_output_predicate() {
        assert_eq!(enabled(), crate::json_output::human_output_enabled());
    }

    /// Under the test defaults (no runtime_flags::init), the non-panicking
    /// accessors report neither json nor quiet, so human output is on.
    #[test]
    fn defaults_allow_human_output() {
        assert!(enabled(), "uninitialized flags must not suppress output");
    }

    /// The modules that run under `--json` must not contain a bare
    /// `println!`. stdout in JSON mode carries only machine-readable events,
    /// so any prose written there corrupts the stream — and a guard that has
    /// to be remembered at each call site is exactly what failed before
    /// (12 guards across ~445 sites).
    ///
    /// Scoped to the build path rather than the whole tree: command handlers
    /// (`tools list`, `sloc`, …) legitimately `println!` their own JSON
    /// document under `--json`, and that document *is* the output.
    ///
    /// If this fires, route the line through `output::info` / `detail` /
    /// `warn` / `error` instead of adding another hand-written guard.
    #[test]
    fn build_path_has_no_bare_println() {
        // Paths are relative to the crate root, which is where cargo runs
        // the test binary's build; CARGO_MANIFEST_DIR makes it independent
        // of the working directory.
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let build_path_modules = [
            "executor/mod.rs",
            "executor/execution.rs",
            "executor/handlers.rs",
            "executor/policy.rs",
            "object_store/mod.rs",
            "object_store/blobs.rs",
            "object_store/descriptors.rs",
            "object_store/restore.rs",
            "object_store/operations.rs",
        ];

        let mut offenders = Vec::new();
        for rel in build_path_modules {
            let path = root.join(rel);
            let src = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
            for (i, line) in src.lines().enumerate() {
                let trimmed = line.trim_start();
                // Skip comments — several of these lines *describe* the rule.
                if trimmed.starts_with("//") {
                    continue;
                }
                if trimmed.contains("println!(") && !trimmed.contains("eprintln!(") {
                    offenders.push(format!("{rel}:{}: {}", i + 1, trimmed));
                }
            }
        }

        assert!(
            offenders.is_empty(),
            "bare println! on the build path corrupts --json stdout; \
             use output::info/detail instead:\n{}",
            offenders.join("\n"),
        );
    }
}
