mod checkers;
mod explicit;
pub mod generators;
mod creators;
pub mod lua;

use anyhow::{Context, Result};
use serde::Serialize;
#[cfg(debug_assertions)]
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;

use crate::color;
use crate::config::{
    output_config_hash, resolve_extra_inputs,
    CheckerConfigWithCommand, SimpleCheckerParams, StandardConfig,
};
use crate::file_index::FileIndex;
use crate::graph::{BuildGraph, Product};

/// Processor name constants for the handful of processors referenced by name
/// in core code. The authoritative plugin registry is
/// `src/registries/processor.rs`, populated at link time via
/// `inventory::submit!` from each processor file.
pub mod names {
    pub const TERA: &str = "tera";
    pub const CC_SINGLE_FILE: &str = "cc_single_file";
    pub const CARGO: &str = "cargo";
    pub const CLIPPY: &str = "clippy";
    pub const SCRIPT: &str = "script";
    pub const GENERATOR: &str = "generator";
    pub const EXPLICIT: &str = "explicit";
}

/// Resolve a relative path against an anchor directory.
/// If the anchor directory is empty, the relative path is returned as-is.
pub fn resolve_anchor_path(anchor_dir: &Path, rel: &str) -> PathBuf {
    if anchor_dir.as_os_str().is_empty() {
        PathBuf::from(rel)
    } else {
        anchor_dir.join(rel)
    }
}

/// The directory containing `path`, as a relative path.
///
/// A path with no parent (a bare filename) yields `.`, so the result is
/// always usable as a directory to join against or pass as a working
/// directory. This idiom — `path.parent().unwrap_or(Path::new("."))` —
/// appeared at 20+ sites; naming it says what the fallback means instead
/// of leaving a bare `"."` at each one.
pub fn parent_dir(path: &Path) -> &Path {
    path.parent().unwrap_or_else(|| Path::new("."))
}

/// The directory containing `path`, as an empty path when there is none.
///
/// Distinct from [`parent_dir`]: callers that feed the result to
/// [`resolve_anchor_path`] rely on "" meaning "no anchor, use the relative
/// path as-is", where "." would produce a `./`-prefixed path instead.
pub fn parent_dir_or_empty(path: &Path) -> &Path {
    path.parent().unwrap_or_else(|| Path::new(""))
}


// Thread-local holding the current processor's declared tools.
// Set before execute()/execute_batch() and cleared after.
// Used by debug_assert in run_command_inner() to catch undeclared tool usage.
#[cfg(debug_assertions)]
thread_local! {
    static DECLARED_TOOLS: RefCell<Option<Vec<String>>> = const { RefCell::new(None) };
}

/// Set the declared tools for the current thread (debug builds only).
#[cfg(debug_assertions)]
pub fn set_declared_tools(tools: Option<Vec<String>>) {
    DECLARED_TOOLS.with(|dt| {
        *dt.borrow_mut() = tools;
    });
}

/// No-op in release builds.
#[cfg(not(debug_assertions))]
pub fn set_declared_tools(_tools: Option<Vec<String>>) {}

/// Temporarily suspend the declared-tools check for user-specified commands.
/// Returns a guard that restores the previous value when dropped.
#[cfg(debug_assertions)]
pub fn suspend_tool_check() -> ToolCheckGuard {
    let prev = DECLARED_TOOLS.with(|dt| dt.borrow_mut().take());
    ToolCheckGuard { prev }
}

/// No-op in release builds.
#[cfg(not(debug_assertions))]
pub const fn suspend_tool_check() -> ToolCheckGuard {
    ToolCheckGuard { _private: () }
}

/// RAII guard that restores the declared tools when dropped.
#[cfg(debug_assertions)]
pub struct ToolCheckGuard {
    prev: Option<Vec<String>>,
}

#[cfg(debug_assertions)]
impl Drop for ToolCheckGuard {
    fn drop(&mut self) {
        DECLARED_TOOLS.with(|dt| {
            *dt.borrow_mut() = self.prev.take();
        });
    }
}

/// No-op guard for release builds.
#[cfg(not(debug_assertions))]
pub struct ToolCheckGuard {
    _private: (),
}

/// Format a `Command` as a shell-like string for display.
pub fn format_command(cmd: &Command) -> String {
    let program = cmd.get_program().to_string_lossy();
    let args: Vec<_> = cmd.get_args()
        .map(|a| a.to_string_lossy())
        .collect();
    if args.is_empty() {
        program.into_owned()
    } else {
        format!("{} {}", program, args.join(" "))
    }
}

/// If --show-child-processes is enabled, print the command that is about to be executed.
pub fn log_command(cmd: &Command) {
    if crate::runtime_flags::show_child_processes() {
        let cwd = cmd.get_current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        if cwd.is_empty() {
            eprintln!("{} {}", color::dim("[exec]"), format_command(cmd));
        } else {
            let cwd_info = format!("(in {cwd})");
            eprintln!("{} {} {}", color::dim("[exec]"), format_command(cmd), color::dim(&cwd_info));
        }
    }
}

/// Shared inner function for running commands interruptibly using tokio.
///
/// - `inherit_stdio`: if true, inherit stdout/stderr (for --show-output mode);
///   if false, always capture via pipes.
/// - `timeout`: if Some, kill the child and return an error if it runs longer than this.
/// - `stdin_data`: if Some, piped to the child's stdin concurrently with
///   draining its output. Feeding stdin fully before reading deadlocks as
///   soon as the child fills the pipe buffer, so the write is driven by the
///   same async runtime that drains stdout/stderr.
fn run_command_inner(
    ctx: &crate::build_context::BuildContext,
    cmd: &Command,
    inherit_stdio: bool,
    timeout: Option<Duration>,
    stdin_data: Option<&[u8]>,
) -> Result<Output> {
    log_command(cmd);

    #[cfg(debug_assertions)]
    DECLARED_TOOLS.with(|dt| {
        if let Some(ref tools) = *dt.borrow() {
            let program = cmd.get_program().to_string_lossy();
            let basename = program.rsplit('/').next().unwrap_or(&program);
            assert!(
                tools.iter().any(|t| {
                    let t_basename = t.rsplit('/').next().unwrap_or(t);
                    t_basename == basename
                }),
                "Processor executed undeclared tool '{basename}'. Declared: {tools:?}",
            );
        }
    });

    if ctx.is_interrupted() {
        anyhow::bail!("Interrupted");
    }

    let program = cmd.get_program().to_os_string();
    let args: Vec<_> = cmd.get_args().map(std::ffi::OsStr::to_os_string).collect();
    let current_dir = cmd.get_current_dir().map(std::path::Path::to_path_buf);
    let envs: Vec<_> = cmd.get_envs()
        .filter_map(|(k, v)| v.map(|val| (k.to_os_string(), val.to_os_string())))
        .collect();

    ctx.runtime().block_on(async {
        let mut tokio_cmd = tokio::process::Command::new(&program);
        tokio_cmd.args(&args);
        if let Some(dir) = &current_dir {
            tokio_cmd.current_dir(dir);
        }
        for (key, val) in &envs {
            tokio_cmd.env(key, val);
        }

        if inherit_stdio {
            tokio_cmd.stdout(std::process::Stdio::inherit());
            tokio_cmd.stderr(std::process::Stdio::inherit());
        } else {
            tokio_cmd.stdout(std::process::Stdio::piped());
            tokio_cmd.stderr(std::process::Stdio::piped());
        }
        if stdin_data.is_some() {
            tokio_cmd.stdin(std::process::Stdio::piped());
        }
        tokio_cmd.kill_on_drop(true);

        let mut child = tokio_cmd.spawn()
            .with_context(|| {
                let prog = program.to_string_lossy();
                let total_len: usize = prog.len() + args.iter().map(|a| a.len() + 1).sum::<usize>();
                let arg_count = args.len();
                if total_len > 100_000 {
                    format!(
                        "Failed to spawn '{prog}' with {arg_count} arguments (total command length ~{total_len} bytes). \
                         This usually means the argument list is too long for the OS (E2BIG). \
                         Consider reducing the number of files or excluding directories."
                    )
                } else {
                    format!("Failed to spawn: {} {}", prog,
                        args.iter().map(|a| a.to_string_lossy()).collect::<Vec<_>>().join(" "))
                }
            })?;

        let mut interrupt_rx = ctx.interrupt_receiver();

        if ctx.is_interrupted() {
            anyhow::bail!("Interrupted");
        }

        // Drive the stdin write concurrently with draining the child's
        // output. A failed write is not fatal on its own — a tool that
        // exits early (or ignores stdin) closes the pipe and gives EPIPE,
        // which is normal — so it is reported only when the child also
        // failed, where truncated input is the likely cause.
        let stdin_pipe = child.stdin.take();
        let stdin_write = async move {
            match (stdin_data, stdin_pipe) {
                (Some(data), Some(mut pipe)) => {
                    use tokio::io::AsyncWriteExt;
                    let result = pipe.write_all(data).await;
                    // Drop closes the pipe, which is what signals EOF to the
                    // child; without it a reader like `aspell list` blocks
                    // forever waiting for more input.
                    drop(pipe);
                    result
                }
                _ => Ok(()),
            }
        };

        let wait = async {
            let (write_result, output) = tokio::join!(stdin_write, child.wait_with_output());
            let output = crate::errors::ctx(output, "Failed to wait for child process")?;
            if !output.status.success() {
                write_result.with_context(|| format!(
                    "Failed to write to stdin of: {}", program.to_string_lossy()
                ))?;
            }
            Ok(output)
        };

        if let Some(dur) = timeout { tokio::select! {
            biased;

            _ = interrupt_rx.changed() => {
                anyhow::bail!("Interrupted")
            }
            () = tokio::time::sleep(dur) => {
                let prog = program.to_string_lossy();
                let args_str = args.iter().map(|a| a.to_string_lossy()).collect::<Vec<_>>().join(" ");
                anyhow::bail!(
                    "Command timed out after {}s and was killed: {} {}",
                    dur.as_secs(), prog, args_str
                )
            }
            result = wait => result,
        } } else { tokio::select! {
            biased;

            _ = interrupt_rx.changed() => {
                anyhow::bail!("Interrupted")
            }
            result = wait => result,
        } }
    })
}

pub fn run_command(ctx: &crate::build_context::BuildContext, cmd: &Command) -> Result<Output> {
    let show = crate::runtime_flags::show_output();
    run_command_inner(ctx, cmd, show, None, None)
}

pub fn run_command_with_timeout(ctx: &crate::build_context::BuildContext, cmd: &Command, timeout: Duration) -> Result<Output> {
    let show = crate::runtime_flags::show_output();
    run_command_inner(ctx, cmd, show, Some(timeout), None)
}

pub fn run_command_capture(ctx: &crate::build_context::BuildContext, cmd: &Command) -> Result<Output> {
    run_command_inner(ctx, cmd, false, None, None)
}

/// Run a command, feeding `stdin_data` to its standard input and capturing
/// its output.
///
/// Exists so processors that pipe content to a tool (aspell) don't have to
/// hand-roll a spawn, which is how they used to lose interrupt handling,
/// `kill_on_drop`, `log_command` and the declared-tools debug check.
pub fn run_command_with_stdin(
    ctx: &crate::build_context::BuildContext,
    cmd: &Command,
    stdin_data: &[u8],
) -> Result<Output> {
    run_command_inner(ctx, cmd, false, None, Some(stdin_data))
}


/// Check that a command exited successfully.
/// On failure, includes any captured stdout/stderr in the error message for debugging.
pub fn check_command_output(output: &Output, context: impl std::fmt::Display) -> Result<()> {
    if !output.status.success() {
        use std::fmt::Write;
        let mut msg = format!("{context} failed");
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        if !stderr.is_empty() {
            let _ = write!(msg, "\nstderr:\n{}", stderr.trim_end());
        }
        if !stdout.is_empty() {
            let _ = write!(msg, "\nstdout:\n{}", stdout.trim_end());
        }
        anyhow::bail!("{msg}");
    }
    Ok(())
}

/// Compute a stub path for a source file.
/// Maps `a/b/file.ext` -> `stub_dir/a_b_file.ext.suffix`.
/// Source path is already relative to project root.
pub fn stub_path(stub_dir: &Path, source: &Path, suffix: &str) -> PathBuf {
    let stub_name = format!(
        "{}.{}",
        source.display().to_string().replace(['/', '\\'], "_"),
        suffix,
    );
    stub_dir.join(stub_name)
}

/// Convert a DOT graph string to SVG using the `dot` command.
///
/// The single implementation: `BuildGraph::to_svg` used to carry a
/// byte-for-byte duplicate of this, each with its own hand-rolled spawn.
pub fn dot_to_svg(ctx: &crate::build_context::BuildContext, dot_content: &str) -> Result<String> {
    use std::process::Command;
    let mut cmd = Command::new("dot");
    cmd.arg("-Tsvg");
    let output = run_command_with_stdin(ctx, &cmd, dot_content.as_bytes())
        .context("Failed to run Graphviz 'dot'. Install Graphviz to use SVG format")?;
    check_command_output(&output, "dot")?;
    String::from_utf8(output.stdout).context("Graphviz 'dot' produced non-UTF-8 SVG output")
}

/// Append new words to a words file without truncating existing content.
/// Used by aspell and zspell processors for their `auto_add_words` feature.
/// `existing` is the set of words already on disk, `new_words` the words to add.
/// If `header_line` is Some and the file does not yet exist, it is written as the
/// first line (e.g. aspell .pws header). New words are appended to the end of the
/// file so that existing content is never lost.
pub fn flush_words(
    existing: &HashSet<String>,
    new_words: &HashSet<String>,
    words_path: &Path,
    header_line: Option<&str>,
) -> Result<()> {
    let to_add: Vec<_> = new_words.iter()
        .filter(|w| !existing.contains(*w))
        .collect();
    if to_add.is_empty() {
        return Ok(());
    }
    let mut sorted: Vec<_> = to_add;
    sorted.sort();

    // If the file doesn't exist yet, create it with the header line.
    // Otherwise just append — never truncate.
    let file_exists = words_path.exists();
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(words_path)
        .with_context(|| format!("Failed to open words file: {}", words_path.display()))?;
    if !file_exists
        && let Some(header) = header_line {
            writeln!(file, "{header}")
                .with_context(|| format!("Failed to write header to words file: {}", words_path.display()))?;
    }
    for word in &sorted {
        writeln!(file, "{word}")
            .with_context(|| format!("Failed to append word to words file: {}", words_path.display()))?;
    }
    if crate::json_output::human_output_enabled() {
        println!("Added {} word(s) to {}", sorted.len(), words_path.display());
    }
    Ok(())
}

/// Check if a config file exists and return it as extra inputs for discover.
/// Used by processors that auto-detect config files (e.g. mypy.ini, .pylintrc).
pub fn config_file_inputs(path: &str) -> Vec<String> {
    if Path::new(path).exists() {
        vec![path.to_string()]
    } else {
        Vec::new()
    }
}

/// Create the parent directory of an output path if it doesn't exist.
/// Used by generator processors before writing output files.
pub fn ensure_output_dir(output: &Path) -> Result<()> {
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create output directory: {}", parent.display()))?;
    }
    Ok(())
}

/// Remove the `output_dirs` of a product. Used by creator `clean()` methods.
/// Returns the number of directories removed.
pub fn clean_output_dir(product: &Product, processor_name: &str, verbose: bool) -> Result<usize> {
    let mut count = 0;
    for output_dir in &product.output_dirs {
        if output_dir.exists() {
            if verbose {
                println!("Removing {} output directory: {}", processor_name, output_dir.display());
            }
            crate::errors::ctx(fs::remove_dir_all(output_dir.as_ref()), &format!("Failed to remove output directory: {}", output_dir.display()))?;
            count += 1;
        }
    }
    Ok(count)
}

/// Build the input list for creators: anchor first, then sibling files
/// (excluding the anchor to avoid duplicates), then extra inputs.
pub fn build_anchor_inputs(anchor: &Path, sibling_files: &[PathBuf], extra: &[PathBuf]) -> Vec<PathBuf> {
    let mut inputs: Vec<PathBuf> = Vec::with_capacity(1 + sibling_files.len() + extra.len());
    inputs.push(anchor.to_path_buf());
    for file in sibling_files {
        if *file != anchor {
            inputs.push(file.clone());
        }
    }
    inputs.extend_from_slice(extra);
    inputs
}

/// Scan and skip-if-empty, the pattern creators repeat in their `discover()`
/// methods. Returns None if no files were found, otherwise the file list.
pub fn scan_or_skip(scan: &crate::config::StandardConfig, file_index: &FileIndex) -> Option<Vec<PathBuf>> {
    let files = file_index.scan(scan, true);
    if files.is_empty() {
        return None;
    }
    Some(files)
}

/// Clean outputs for a product: remove each output file.
/// When `verbose` is true, prints a message for each removed file.
/// Returns the number of files removed.
pub fn clean_outputs(product: &Product, label: &str, verbose: bool) -> Result<usize> {
    let mut count = 0;
    for output in &product.outputs {
        match fs::remove_file(output) {
            Ok(()) => {
                count += 1;
                if verbose {
                    println!("Removed {} output: {}", label, output.display());
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(anyhow::Error::from(e)
                .context(format!("Failed to remove {} output: {}", label, output.display()))),
        }
    }
    Ok(count)
}

/// Options for filtering sibling files in directory-based product discovery.
#[derive(Debug)]
pub struct SiblingFilter<'a> {
    pub extensions: &'a [&'a str],
    pub excludes: &'a [&'a str],
}

/// Options for `discover_directory_products`.
pub struct DirectoryProductOpts<'a, H: serde::Serialize> {
    pub scan: &'a crate::config::StandardConfig,
    pub file_index: &'a FileIndex,
    pub dep_inputs: &'a [String],
    pub cfg_hash: &'a H,
    /// Allowlist of field names to include in the config-change checksum.
    pub checksum_fields: &'static [&'static str],
    pub siblings: &'a SiblingFilter<'a>,
    pub processor_name: &'a str,
    pub output_dir_name: Option<&'a str>,
}

/// Discover directory-based products: each discovered file anchors a product whose inputs
/// include all sibling files under the same directory (filtered by extensions/excludes).
///
/// Used by processors like `make` and `cargo` where a manifest file (Makefile, Cargo.toml)
/// represents a build unit and all files in its directory are inputs.
/// All paths are relative to project root.
///
/// When `output_dir_name` is `Some("dir_name")`, the product gets an `output_dir` set to
/// `anchor_parent/dir_name`, enabling directory-level caching for creators.
pub fn discover_directory_products(
    graph: &mut BuildGraph,
    opts: DirectoryProductOpts<'_, impl serde::Serialize>,
) -> Result<()> {
    let DirectoryProductOpts { scan, file_index, dep_inputs, cfg_hash, checksum_fields, siblings, processor_name, output_dir_name } = opts;
    let files = file_index.scan(scan, true);
    if files.is_empty() {
        return Ok(());
    }

    let hash = Some(output_config_hash(cfg_hash, checksum_fields));
    let extra = resolve_extra_inputs(dep_inputs)?;

    for anchor in files {
        let anchor_dir = anchor.parent().map(std::path::Path::to_path_buf).unwrap_or_default();

        // Collect all matching sibling files under the anchor's directory as inputs
        let sibling_files = file_index.query(
            &anchor_dir,
            siblings.extensions,
            siblings.excludes,
            &[],
            &[],
            &[],
        );

        let inputs = build_anchor_inputs(&anchor, &sibling_files, &extra);

        if let Some(dir_name) = output_dir_name {
            let output_dir = if anchor_dir.as_os_str().is_empty() {
                PathBuf::from(dir_name)
            } else {
                anchor_dir.join(dir_name)
            };
            graph.add_product_with_output_dir(inputs, vec![], processor_name, hash.clone(), output_dir)?;
        } else {
            // Empty outputs: cache entry = success record
            graph.add_product(inputs, vec![], processor_name, hash.clone())?;
        }
    }

    Ok(())
}

/// Discover checker products: one product per source file, empty outputs.
///
/// This is the single entry point for checker discovery. It merges
/// `dep_inputs` (explicit extra inputs) with `dep_auto` (config file
/// patterns like `"ruff.toml"` that expand to existing files). Both are
/// from `StandardConfig`.
///
/// Replaces the former `discover_checker_products` (no `dep_auto` merge)
/// and `checker_discover` (with `dep_auto` merge) — the split was a
/// correctness hazard since picking the wrong one silently lost `dep_auto`.
/// 8 arguments against clippy's limit of 7. `dep_inputs` and `dep_auto` look
/// redundant with `scan` but are not — `terms` and `zspell` pass computed
/// lists rather than the config's own, so folding them into `scan` would
/// silently change what those two processors depend on.
#[allow(clippy::too_many_arguments)]
pub fn discover_checker_products(
    graph: &mut BuildGraph,
    scan: &crate::config::StandardConfig,
    file_index: &FileIndex,
    dep_inputs: &[String],
    dep_auto: &[String],
    cfg_hash: &impl serde::Serialize,
    checksum_fields: &[&str],
    processor_name: &str,
) -> Result<()> {
    let files = file_index.scan(scan, true);
    if files.is_empty() {
        return Ok(());
    }
    let mut all_dep_inputs = dep_inputs.to_vec();
    for ai in dep_auto {
        all_dep_inputs.extend(config_file_inputs(ai));
    }
    let hash = Some(output_config_hash(cfg_hash, checksum_fields));
    let extra = resolve_extra_inputs(&all_dep_inputs)?;
    for file in files {
        let mut inputs = Vec::with_capacity(1 + extra.len());
        inputs.push(file);
        inputs.extend_from_slice(&extra);
        graph.add_product(inputs, vec![], processor_name, hash.clone())?;
    }
    Ok(())
}

/// Standard checker `auto_detect`: check if scan finds any files.
/// When `check_scan_root` is true, also validates that the scan root
/// directory exists (used by processors like `clang_tidy` that have a
/// meaningful `scan_root` guard).
pub fn checker_auto_detect(scan: &crate::config::StandardConfig, file_index: &FileIndex) -> bool {
    !file_index.scan(scan, true).is_empty()
}

/// Run a command in the parent directory of an anchor file (e.g., Makefile, Cargo.toml).
/// Sets `current_dir` to the parent directory (unless it's the project root).
/// Returns a display-friendly directory name for error messages.
pub fn run_in_anchor_dir(ctx: &crate::build_context::BuildContext, cmd: &mut Command, anchor: &Path) -> Result<Output> {
    let anchor_dir = anchor.parent()
        .context("Anchor file has no parent directory")?;
    if !anchor_dir.as_os_str().is_empty() {
        cmd.current_dir(anchor_dir);
    }
    run_command(ctx, cmd)
}

/// Format the parent directory of an anchor file for display.
/// Returns `"."` for root-level files.
pub fn anchor_display_dir(anchor: &Path) -> &str {
    anchor.parent()
        .and_then(|p| if p.as_os_str().is_empty() { None } else { p.to_str() })
        .unwrap_or(".")
}

/// Ensure a stub directory exists, creating it if necessary.
pub fn ensure_stub_dir(stub_dir: &Path, processor_name: &str) -> Result<()> {
    if !stub_dir.exists() {
        fs::create_dir_all(stub_dir)
            .with_context(|| format!("Failed to create {processor_name} stub directory"))?;
    }
    Ok(())
}

/// Run a checker tool on one or more files.
///
/// Builds a command from the tool name, optional subcommand, config args, and file paths,
/// then runs it and checks the output. `max_arg_len` is the threshold (in bytes) at
/// which the invocation is split into multiple calls; it is sourced from
/// `build.max_arg_len` in rsconstruct.toml.
pub fn run_checker(
    ctx: &crate::build_context::BuildContext,
    tool: &str,
    subcommand: Option<&str>,
    args: &[String],
    files: &[&Path],
    max_arg_len: usize,
) -> Result<()> {
    let mut files: Vec<&Path> = files.to_vec();
    files.sort();
    files.dedup();
    let files = &files[..];

    let base_len: usize = tool.len()
        + subcommand.map_or(0, |s| s.len() + 1)
        + args.iter().map(|a| a.len() + 1).sum::<usize>();

    let files_len: usize = files.iter().map(|f| f.as_os_str().len() + 1).sum();
    if base_len + files_len <= max_arg_len {
        return run_checker_once(ctx, tool, subcommand, args, files);
    }

    for (start, end) in checker_chunk_ranges(base_len, files, max_arg_len) {
        run_checker_once(ctx, tool, subcommand, args, &files[start..end])?;
    }
    Ok(())
}

/// Greedily pack `files` into chunks whose command line stays within
/// `max_arg_len`, returning half-open (start, end) index ranges. Every chunk
/// re-pays `base_len` (tool + subcommand + config args). A single path longer
/// than the limit still gets its own over-limit chunk so packing always makes
/// progress.
fn checker_chunk_ranges(base_len: usize, files: &[&Path], max_arg_len: usize) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut chunk_start = 0;
    while chunk_start < files.len() {
        let mut chunk_len = base_len;
        let mut chunk_end = chunk_start;
        while chunk_end < files.len() {
            let file_len = files[chunk_end].as_os_str().len() + 1;
            if chunk_len + file_len > max_arg_len && chunk_end > chunk_start {
                break;
            }
            chunk_len += file_len;
            chunk_end += 1;
        }
        ranges.push((chunk_start, chunk_end));
        chunk_start = chunk_end;
    }
    ranges
}

fn run_checker_once(
    ctx: &crate::build_context::BuildContext,
    tool: &str,
    subcommand: Option<&str>,
    args: &[String],
    files: &[&Path],
) -> Result<()> {
    let mut cmd = Command::new(tool);
    if let Some(sub) = subcommand {
        cmd.arg(sub);
    }
    for arg in args {
        cmd.arg(arg);
    }
    for file in files {
        cmd.arg(file);
    }
    let output = run_command(ctx, &cmd)?;
    check_command_output(&output, tool)
}

/// Per-file batch execution for in-process checkers: each product gets its
/// own result, so under --keep-going one bad file fails only its own product
/// instead of the whole chunk (the processor contract requires per-file
/// results from internal processors).
pub fn execute_checker_batch_per_file<F>(
    products: &[&Product],
    check_fn: F,
) -> Vec<Result<()>>
where
    F: Fn(&Path) -> Result<()>,
{
    products.iter().map(|p| check_fn(p.primary_input())).collect()
}

/// Single-invocation batch execution for external-tool checkers. The tool
/// runs once over all files, so its exit status necessarily applies to the
/// whole chunk.
pub fn execute_checker_batch<F>(
    ctx: &crate::build_context::BuildContext,
    products: &[&Product],
    batch_fn: F,
) -> Vec<Result<()>>
where
    F: Fn(&crate::build_context::BuildContext, &[&Path]) -> Result<()>,
{
    let input_paths: Vec<&Path> = products.iter()
        .map(|p| p.primary_input())
        .collect();

    match batch_fn(ctx, &input_paths) {
        Ok(()) => products.iter().map(|_| Ok(())).collect(),
        Err(e) => {
            let err_msg = e.to_string();
            products.iter().map(|_| Err(anyhow::anyhow!("{err_msg}"))).collect()
        }
    }
}

pub fn execute_generator_batch<F>(
    ctx: &crate::build_context::BuildContext,
    products: &[&Product],
    batch_fn: F,
) -> Vec<Result<()>>
where
    F: Fn(&crate::build_context::BuildContext, &[(&Path, &Path)]) -> Result<()>,
{
    let pairs: Vec<(&Path, &Path)> = products.iter()
        .map(|p| (p.primary_input(), p.primary_output()))
        .collect();

    match batch_fn(ctx, &pairs) {
        Ok(()) => products.iter().map(|_| Ok(())).collect(),
        Err(e) => {
            let err_msg = e.to_string();
            products.iter().map(|_| Err(anyhow::anyhow!("{err_msg}"))).collect()
        }
    }
}

pub use checkers::terms;
pub use generators::tags as tags_cmd;
pub use lua::LuaProcessor;

/// Map from processor name to processor instance. Used throughout the build pipeline.
pub type ProcessorMap = HashMap<String, Box<dyn Processor>>;

/// The type of processor - whether it generates new files, checks existing files,
/// or produces a mass of output files in a directory.
///
/// # Caching Behavior
///
/// All processor types use the cache to avoid redundant work:
///
/// - **Generators** produce output files (e.g., executables, rendered templates). The cache
///   stores copies of these outputs. On `rsconstruct clean`, output files are deleted but the cache
///   remains intact. On the next `rsconstruct build`, outputs are restored from cache (fast copy/hardlink)
///   instead of being regenerated.
///
/// - **Checkers** validate input files but produce no output files. The cache entry itself
///   serves as a "success marker". On `rsconstruct clean`, there's nothing to delete. On the next
///   `rsconstruct build`, if the cache entry exists and inputs haven't changed, the check is skipped
///   entirely (instant).
///
/// - **Creators** produce a mass of output files in a directory but don't enumerate
///   those outputs individually (e.g., pip → site-packages, npm → `node_modules`, cargo → target).
///   They use stamp files or empty outputs for cache tracking, similar to checkers.
///
/// This design ensures that `rsconstruct clean && rsconstruct build` is fast for all types - generators
/// restore from cache, checkers skip entirely, creators re-run only when inputs change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[derive(strum::EnumIter)]
pub enum ProcessorType {
    /// Generates new output files from input files (e.g., tera, `cc_single_file`).
    /// Products have non-empty `outputs` which are cached and can be restored.
    Generator,
    /// Checks/validates input files without producing output files (e.g., ruff, pylint, shellcheck).
    /// Products have empty `outputs`; the cache entry serves as the success marker.
    Checker,
    /// Runs a command and caches declared output files and directories
    /// (e.g., cargo, pip, npm, sphinx, mdbook, user-defined creators).
    Creator,
    /// Many inputs aggregated into (possibly) many output files and/or directories.
    /// Unlike Generator (one product per input file), creates a single product.
    Explicit,
    /// A user-defined processor implemented in Lua via the plugin runtime.
    /// Lua scripts may override this by declaring `processor_type()` returning
    /// "checker"/"generator"/"creator"/"explicit" — only scripts that omit that
    /// function are categorized as Lua.
    Lua,
}

impl ProcessorType {
    /// Returns the string representation
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Generator => "generator",
            Self::Checker => "checker",
            Self::Creator => "creator",
            Self::Explicit => "explicit",
            Self::Lua => "lua",
        }
    }

    /// Returns a human-readable description of this processor type.
    pub const fn description(self) -> &'static str {
        match self {
            Self::Generator => "Generates output files from input files (1 input -> 1 output per format)",
            Self::Checker => "Validates input files without producing outputs",
            Self::Creator => "Runs a command and caches declared output files and directories",
            Self::Explicit => "Many inputs aggregated into (possibly) many output files and/or directories",
            Self::Lua => "User-defined processor implemented in Lua via the plugin runtime",
        }
    }

}

/// Helper namespace for processor boilerplate (`config_json`, clean, `clean_output_dir`).
/// No state — description and `processor_type` now live on the plugin metadata.
pub struct ProcessorBase;

impl ProcessorBase {
    pub fn config_json<C: Serialize>(config: &C) -> Option<String> {
        serde_json::to_string(config).ok()
    }

    pub fn clean(product: &Product, name: &str, verbose: bool) -> anyhow::Result<usize> {
        clean_outputs(product, name, verbose)
    }

    pub fn clean_output_dir(product: &Product, name: &str, verbose: bool) -> anyhow::Result<usize> {
        clean_output_dir(product, name, verbose)
    }
}

/// Trait for processors that can discover products for the build graph.
///
/// Processors come in three types (see [`ProcessorType`]):
/// - **Generators**: Create output files from inputs (must override `clean()`)
/// - **Checkers**: Validate inputs without producing outputs (use default `clean()`)
/// - **Creators**: Produce a mass of output files in a directory without enumerating them
///
/// # Implementing a Checker
///
/// Checkers are simpler - just implement the required methods and use defaults for the rest:
///
/// ```ignore
/// impl Processor for MyChecker {
///     fn description(&self) -> &str { "Check files with mytool" }
///     fn discover(&self, graph: &mut BuildGraph, file_index: &FileIndex, instance_name: &str) -> Result<()> {
///         discover_checker_products(graph, ..., instance_name)  // empty outputs
///     }
///     fn execute(&self, product: &Product) -> Result<()> {
///         run_mytool(product.primary_input())
///     }
///     fn auto_detect(&self, file_index: &FileIndex) -> bool { ... }
/// }
/// ```
///
/// # Implementing a Generator
///
/// Generators must override `processor_type()` and `clean()`:
///
/// ```ignore
/// impl Processor for MyGenerator {
///     fn description(&self) -> &str { "Generate files" }
///     fn processor_type(&self) -> ProcessorType { ProcessorType::Generator }
///     fn discover(&self, graph: &mut BuildGraph, file_index: &FileIndex, instance_name: &str) -> Result<()> {
///         graph.add_product(inputs, outputs, instance_name, ...)?;  // non-empty outputs
///     }
///     fn execute(&self, product: &Product) -> Result<()> { ... }
///     fn clean(&self, product: &Product, verbose: bool) -> Result<usize> {
///         clean_outputs(product, &product.processor, verbose)
///     }
///     fn auto_detect(&self, file_index: &FileIndex) -> bool { ... }
/// }
/// ```
///
/// Must be Sync + Send for parallel execution support.
pub trait Processor: Sync + Send {

    /// Access the standard config fields shared by every processor.
    ///
    /// Required, and the single accessor: there used to be a second,
    /// `standard_config() -> Option<&StandardConfig>`, which every one of
    /// its 27 implementations returned the same `&self.config.standard`
    /// from. The `Option` meant the `discover` default had to `.expect()` —
    /// a required method wearing a default's clothes, which panicked at
    /// runtime for any processor that implemented one accessor but not the
    /// other.
    fn scan_config(&self) -> &crate::config::StandardConfig;

    /// Discover all products this processor can produce.
    /// Default: standard checker discover using `dep_inputs/dep_auto` from `scan_config`.
    fn discover(&self, graph: &mut BuildGraph, file_index: &FileIndex, instance_name: &str) -> Result<()> {
        let cfg = self.scan_config();
        discover_checker_products(graph, cfg, file_index, &cfg.dep_inputs, &cfg.dep_auto, cfg, <crate::config::StandardConfig as crate::config::KnownFields>::checksum_fields(), instance_name)
    }

    /// Discover products for clean operation (outputs only, skip expensive dependency scanning).
    fn discover_for_clean(&self, graph: &mut BuildGraph, file_index: &FileIndex, instance_name: &str) -> Result<()> {
        self.discover(graph, file_index, instance_name)
    }

    /// Execute a single product
    fn execute(&self, ctx: &crate::build_context::BuildContext, product: &Product) -> Result<()>;

    /// Clean outputs for a product. Checkers: default does nothing. Generators: override.
    fn clean(&self, _product: &Product, _verbose: bool) -> Result<usize> {
        Ok(0)
    }

    /// Auto-detect whether this processor is relevant for the current project.
    /// Default: check if scan finds any files.
    fn auto_detect(&self, file_index: &FileIndex) -> bool {
        checker_auto_detect(self.scan_config(), file_index)
    }

    /// Return the names of external tools required by this processor
    fn required_tools(&self) -> Vec<String> {
        Vec::new()
    }

    /// Return tool version commands: Vec of (`tool_name`, `args_to_get_version`).
    fn tool_version_commands(&self) -> Vec<(String, Vec<String>)> {
        self.required_tools()
            .into_iter()
            .map(|tool| (tool, vec!["--version".to_string()]))
            .collect()
    }

    /// Execute multiple products in one invocation.
    /// Only called when the plugin's `supports_batch` flag is true AND the
    /// user config has `batch = true`.
    fn execute_batch(&self, ctx: &crate::build_context::BuildContext, products: &[&Product]) -> Vec<Result<()>> {
        products.iter().map(|p| self.execute(ctx, p)).collect()
    }

    /// Instance-level fix capability, for processors whose fix support
    /// depends on configuration (e.g. script's `fix_command`). Static
    /// capability comes from the plugin's `can_fix` flag; `rsconstruct fix`
    /// includes a processor when either is true.
    fn config_has_fix(&self) -> bool {
        false
    }

    /// Fix a single product (modify source files in place).
    /// Only called when `can_fix()` returns true.
    fn fix(&self, ctx: &crate::build_context::BuildContext, product: &Product) -> Result<()> {
        let _ = (ctx, product);
        anyhow::bail!("fix not implemented for this processor")
    }

    /// Whether fix mode supports batch execution.
    fn supports_fix_batch(&self) -> bool {
        false
    }

    /// Fix multiple products in one invocation.
    /// Only called when `supports_fix_batch()` returns true.
    fn fix_batch(&self, ctx: &crate::build_context::BuildContext, products: &[&Product]) -> Vec<Result<()>> {
        products.iter().map(|p| self.fix(ctx, p)).collect()
    }

    /// Return the processor's configuration as JSON for config change detection.
    ///
    /// Default: serialize the standard fields. A processor with config
    /// fields of its own must override this (most do) or its extra fields
    /// won't appear in the `config changed` notice.
    fn config_json(&self) -> Option<String> {
        serde_json::to_string(self.scan_config()).ok()
    }
}

/// Central registry of all known external tools — single source of truth for
/// runtime category and install command. Both `tool_install_command()` and
/// `tool_runtime()` (in `builder/tools.rs`) look up data from this table.
///
/// Runtime categories: "python", "node", "ruby", "rust", "perl", "jvm", "system"
///
/// A single way to install a tool.
pub struct InstallMethod {
    /// Package manager or method name (e.g., "pip", "apt", "npm", "snap", "cargo", "binary")
    pub method: &'static str,
    /// Package name for the package manager (e.g., "taplo-cli" for cargo, "texlive-latex-base" for apt)
    pub package: &'static str,
}

/// Context for executing an install: verbosity and eatmydata preference.
///
/// `sudo` is decided at execution time via `crate::platform::needs_sudo()`;
/// callers don't need to pass it in.
pub struct InstallCtx {
    /// When true, print each subprocess argv before running it.
    pub verbose: bool,
    /// When true, prepend `eatmydata` to system-package-manager calls
    /// (apt/dnf/pacman) for faster installs. No-op when eatmydata isn't
    /// on PATH.
    pub use_eatmydata: bool,
}

impl InstallMethod {
    /// Human-readable preview of what installing this entry's package
    /// would do — first line of the first describe step. Lossy for
    /// multi-step installs (binary fetches), but adequate for `tools list`.
    pub fn command(&self) -> String {
        let steps = describe(self.method, &[self.package]);
        steps.first().map_or_else(|| format!("({}) {}", self.method, self.package), |argv| join_argv(argv))
    }
}

/// Display the steps that `run(method, packages, ctx)` would execute,
/// without executing them. Used for logs and previews.
pub fn describe(method: &str, packages: &[&str]) -> Vec<Vec<String>> {
    let sudo = sudo_argv();
    let strs = |parts: &[&str]| -> Vec<String> {
        parts.iter().map(|s| (*s).to_string()).collect()
    };
    let prefix = |head: &[&str], tail: &[&str]| -> Vec<String> {
        sudo.iter().chain(head.iter()).chain(tail.iter())
            .map(|s| (*s).to_string()).collect()
    };
    match method {
        "apt" => {
            // Run `apt-get update` first so the package index isn't stale.
            // CI runners ship with indexes a few days old; without this, an
            // `install` for a security-updated package 404s on the mirror.
            let update = prefix(&["apt-get", "update"], &[]);
            let mut install = prefix(&["apt-get", "install", "-y"], &[]);
            install.extend(packages.iter().map(|s| (*s).to_string()));
            vec![update, install]
        }
        "dnf" => {
            let mut argv = prefix(&["dnf", "install", "-y"], &[]);
            argv.extend(packages.iter().map(|s| (*s).to_string()));
            vec![argv]
        }
        "pacman" => {
            let mut argv = prefix(&["pacman", "-S", "--noconfirm"], &[]);
            argv.extend(packages.iter().map(|s| (*s).to_string()));
            vec![argv]
        }
        "brew" => {
            let mut argv = strs(&["brew", "install"]);
            argv.extend(packages.iter().map(|s| (*s).to_string()));
            vec![argv]
        }
        "snap" => {
            let mut argv = prefix(&["snap", "install"], &[]);
            argv.extend(packages.iter().map(|s| (*s).to_string()));
            vec![argv]
        }
        "pip"   => vec![{ let mut a = strs(&["pip", "install"]); a.extend(packages.iter().map(|s| (*s).to_string())); a }],
        "npm"   => vec![{ let mut a = strs(&["npm", "install", "-g"]); a.extend(packages.iter().map(|s| (*s).to_string())); a }],
        "cargo" => vec![{ let mut a = strs(&["cargo", "install"]); a.extend(packages.iter().map(|s| (*s).to_string())); a }],
        "gem"   => vec![{ let mut a = strs(&["gem", "install"]); a.extend(packages.iter().map(|s| (*s).to_string())); a }],
        "binary" => packages.iter().flat_map(|p| describe_binary(p)).collect(),
        "manual" => packages.iter()
            .map(|p| vec!["# manual:".to_string(), (*p).to_string()])
            .collect(),
        _ => packages.iter()
            .map(|p| vec![format!("# unknown method '{}':", method), (*p).to_string()])
            .collect(),
    }
}

/// Execute the install for `method` over `packages` (batched when the
/// method supports it). Each subprocess runs argv-style — never via a
/// shell. Returns Err on the first failure.
pub fn run(method: &str, packages: &[&str], ctx: &InstallCtx) -> anyhow::Result<()> {
    use anyhow::Context as _;
    use std::process::Command;
    if packages.is_empty() {
        return Ok(());
    }
    let exec = |argv: &[String]| -> anyhow::Result<()> {
        if ctx.verbose {
            println!("Running: {}", join_argv(argv));
        }
        // Deliberately NOT routed through run_command: installers must
        // inherit the terminal, both so `sudo` can prompt for a password and
        // so apt/dnf can render progress. Capturing would hang on the
        // password prompt. This is one of the two documented exceptions to
        // the "everything goes through the central runner" rule (see
        // docs/src/internal/architecture.md).
        let status = Command::new(&argv[0]).args(&argv[1..]).status()
            .with_context(|| format!("failed to spawn: {}", join_argv(argv)))?;
        if !status.success() {
            anyhow::bail!(
                "{} exited with code {}",
                join_argv(argv),
                status.code().map_or_else(|| "unknown".to_string(), |c| c.to_string())
            );
        }
        Ok(())
    };
    let sudo = sudo_argv();
    let eatmydata: &[&str] = if ctx.use_eatmydata && which::which("eatmydata").is_ok() {
        &["eatmydata"]
    } else {
        &[]
    };
    let pkgmgr_argv = |head: &[&str]| -> Vec<String> {
        sudo.iter()
            .chain(eatmydata.iter())
            .chain(head.iter())
            .map(|s| (*s).to_string())
            .collect()
    };
    match method {
        "apt" => {
            // Refresh the package index before installing — CI runners ship
            // with stale indexes that 404 on security-updated packages.
            let update = pkgmgr_argv(&["apt-get", "update"]);
            exec(&update)?;
            let mut argv = pkgmgr_argv(&["apt-get", "install", "-y"]);
            argv.extend(packages.iter().map(|s| (*s).to_string()));
            exec(&argv)
        }
        "dnf" => {
            let mut argv = pkgmgr_argv(&["dnf", "install", "-y"]);
            argv.extend(packages.iter().map(|s| (*s).to_string()));
            exec(&argv)
        }
        "pacman" => {
            let mut argv = pkgmgr_argv(&["pacman", "-S", "--noconfirm"]);
            argv.extend(packages.iter().map(|s| (*s).to_string()));
            exec(&argv)
        }
        "brew" => {
            // brew is macOS-only; no sudo, no eatmydata.
            let mut argv = vec!["brew".to_string(), "install".to_string()];
            argv.extend(packages.iter().map(|s| (*s).to_string()));
            exec(&argv)
        }
        "snap" => {
            // snap doesn't need eatmydata.
            let mut argv: Vec<String> = sudo.iter().chain(["snap", "install"].iter())
                .map(|s| (*s).to_string()).collect();
            argv.extend(packages.iter().map(|s| (*s).to_string()));
            exec(&argv)
        }
        "pip" => {
            let mut argv = vec!["pip".to_string(), "install".to_string()];
            argv.extend(packages.iter().map(|s| (*s).to_string()));
            exec(&argv)
        }
        "npm" => {
            let mut argv = vec!["npm".to_string(), "install".to_string(), "-g".to_string()];
            argv.extend(packages.iter().map(|s| (*s).to_string()));
            exec(&argv)
        }
        "cargo" => {
            let mut argv = vec!["cargo".to_string(), "install".to_string()];
            argv.extend(packages.iter().map(|s| (*s).to_string()));
            exec(&argv)
        }
        "gem" => {
            let mut argv = vec!["gem".to_string(), "install".to_string()];
            argv.extend(packages.iter().map(|s| (*s).to_string()));
            exec(&argv)
        }
        "binary" => {
            for pkg in packages {
                run_binary(pkg, ctx)?;
            }
            Ok(())
        }
        "manual" => anyhow::bail!(
            "method '{}' is manual-only — install these packages by hand: {}",
            method, packages.join(", ")
        ),
        other => anyhow::bail!("unknown install method '{other}'"),
    }
}

fn sudo_argv() -> &'static [&'static str] {
    if crate::platform::needs_sudo() { &["sudo"] } else { &[] }
}

fn join_argv(argv: &[String]) -> String {
    argv.join(" ")
}

/// Description of the steps that `run_binary` would execute. Used for
/// preview/logging only.
fn describe_binary(pkg: &str) -> Vec<Vec<String>> {
    match binary_recipe(pkg) {
        Some(BinaryRecipe { url, archive, dest, .. }) => {
            let tmp = format!("/tmp/{dest}");
            let dl = format!("/tmp/{dest}.dl");
            let final_path = format!("/usr/local/bin/{dest}");
            let download = vec!["curl".to_string(), "-fsSL".to_string(), "-o".to_string(),
                                dl.clone(), url.to_string()];
            let extract = match archive {
                ArchiveKind::TarGz { inner } => vec![
                    "tar".to_string(), "-xzf".to_string(),
                    dl,
                    "-C".to_string(), "/tmp".to_string(),
                    inner.to_string(),
                ],
                ArchiveKind::Gunzip => vec![
                    "gunzip".to_string(), "-f".to_string(),
                    dl,
                ],
                ArchiveKind::Raw => vec![
                    "mv".to_string(), dl, tmp.clone(),
                ],
            };
            let chmod = vec!["chmod".to_string(), "+x".to_string(), tmp.clone()];
            let sudo = sudo_argv();
            let mv = sudo.iter().chain(["mv", &tmp, &final_path].iter())
                .map(|s| (*s).to_string()).collect();
            vec![download, extract, chmod, mv]
        }
        None => vec![vec![format!("# unknown binary recipe '{pkg}'")]],
    }
}

fn run_binary(pkg: &str, ctx: &InstallCtx) -> anyhow::Result<()> {
    use anyhow::Context as _;
    use std::process::Command;
    let recipe = binary_recipe(pkg)
        .ok_or_else(|| anyhow::anyhow!("no binary install recipe for '{pkg}'"))?;
    let download = format!("/tmp/{}.dl", recipe.dest);
    let final_tmp = format!("/tmp/{}", recipe.dest);
    let final_path = format!("/usr/local/bin/{}", recipe.dest);
    let exec = |argv: &[&str]| -> anyhow::Result<()> {
        if ctx.verbose {
            println!("Running: {}", argv.join(" "));
        }
        // Same exception as `run()` above: the binary installer shells out to
        // `sudo install`, which needs the terminal for its password prompt.
        let status = Command::new(argv[0]).args(&argv[1..]).status()
            .with_context(|| format!("failed to spawn: {}", argv.join(" ")))?;
        if !status.success() {
            anyhow::bail!(
                "{} exited with code {}",
                argv.join(" "),
                status.code().map_or_else(|| "unknown".to_string(), |c| c.to_string())
            );
        }
        Ok(())
    };
    exec(&["curl", "-fsSL", "-o", &download, recipe.url])?;
    match recipe.archive {
        ArchiveKind::TarGz { inner } => {
            exec(&["tar", "-xzf", &download, "-C", "/tmp", inner])?;
            // After tar, the inner file is at /tmp/<inner>; rename to final_tmp.
            let inner_path = format!("/tmp/{inner}");
            if inner_path != final_tmp {
                std::fs::rename(&inner_path, &final_tmp)
                    .with_context(|| format!("rename {inner_path} -> {final_tmp}"))?;
            }
            std::fs::remove_file(&download).ok();
        }
        ArchiveKind::Gunzip => {
            // gunzip strips the .gz extension. Our download path ends in
            // .dl; rename to .dl.gz so gunzip leaves /tmp/<dest>.dl, then
            // move to final_tmp.
            let gz = format!("{download}.gz");
            std::fs::rename(&download, &gz).with_context(|| format!("rename {download} -> {gz}"))?;
            exec(&["gunzip", "-f", &gz])?;
            std::fs::rename(&download, &final_tmp)
                .with_context(|| format!("rename {download} -> {final_tmp}"))?;
        }
        ArchiveKind::Raw => {
            std::fs::rename(&download, &final_tmp)
                .with_context(|| format!("rename {download} -> {final_tmp}"))?;
        }
    }
    crate::platform::set_permissions_mode(std::path::Path::new(&final_tmp), 0o755)
        .with_context(|| format!("chmod +x {final_tmp}"))?;
    let sudo = sudo_argv();
    let mut mv: Vec<&str> = sudo.to_vec();
    mv.extend(["mv", &final_tmp, &final_path]);
    exec(&mv)
}

struct BinaryRecipe {
    url: &'static str,
    archive: ArchiveKind,
    dest: &'static str,
}

enum ArchiveKind {
    TarGz { inner: &'static str },
    Gunzip,
    /// The download is the binary itself, no extraction needed.
    Raw,
}

fn binary_recipe(pkg: &str) -> Option<BinaryRecipe> {
    match pkg {
        "rumdl" => Some(BinaryRecipe {
            url: "https://github.com/rvben/rumdl/releases/download/v0.1.81/rumdl-v0.1.81-x86_64-unknown-linux-gnu.tar.gz",
            archive: ArchiveKind::TarGz { inner: "rumdl" },
            dest: "rumdl",
        }),
        "taplo" => Some(BinaryRecipe {
            url: "https://github.com/tamasfe/taplo/releases/latest/download/taplo-linux-x86_64.gz",
            archive: ArchiveKind::Gunzip,
            dest: "taplo",
        }),
        "hadolint" => Some(BinaryRecipe {
            url: "https://github.com/hadolint/hadolint/releases/latest/download/hadolint-Linux-x86_64",
            archive: ArchiveKind::Raw,
            dest: "hadolint",
        }),
        // checkpatch.pl ships only inside the kernel tree — there is no release
        // artifact and no distro package. Fetching the single script from
        // torvalds/linux master is the automatable equivalent of "copy it out
        // of a kernel checkout", and keeps the tool out of the manual-only
        // bucket that `tools install --all` rejects.
        "checkpatch.pl" => Some(BinaryRecipe {
            url: "https://raw.githubusercontent.com/torvalds/linux/master/scripts/checkpatch.pl",
            archive: ArchiveKind::Raw,
            dest: "checkpatch.pl",
        }),
        _ => None,
    }
}

/// Information about an external tool: its name, runtime category, and install methods.
pub struct ToolInfo {
    /// Tool binary name
    pub name: &'static str,
    /// Runtime category ("python", "node", "ruby", "rust", "perl", "jvm", "system")
    pub runtime: &'static str,
    /// Install methods, ordered by preference (first is the default)
    pub install_methods: &'static [InstallMethod],
}

pub static TOOLS: &[ToolInfo] = &[
    // Python tools
    ToolInfo { name: "ruff", runtime: "python", install_methods: &[InstallMethod { method: "pip", package: "ruff" }] },
    ToolInfo { name: "pylint", runtime: "python", install_methods: &[InstallMethod { method: "pip", package: "pylint" }] },
    ToolInfo { name: "mypy", runtime: "python", install_methods: &[InstallMethod { method: "pip", package: "mypy" }] },
    ToolInfo { name: "pyrefly", runtime: "python", install_methods: &[InstallMethod { method: "pip", package: "pyrefly" }] },
    ToolInfo { name: "yamllint", runtime: "python", install_methods: &[InstallMethod { method: "pip", package: "yamllint" }] },
    ToolInfo { name: "sphinx-build", runtime: "python", install_methods: &[InstallMethod { method: "pip", package: "sphinx" }] },
    ToolInfo { name: "pip", runtime: "python", install_methods: &[InstallMethod { method: "apt", package: "python3-pip" }] },
    ToolInfo { name: "jsonlint", runtime: "python", install_methods: &[InstallMethod { method: "pip", package: "demjson3" }] },
    ToolInfo { name: "cpplint", runtime: "python", install_methods: &[InstallMethod { method: "pip", package: "cpplint" }] },
    ToolInfo { name: "black", runtime: "python", install_methods: &[InstallMethod { method: "pip", package: "black" }] },
    ToolInfo { name: "pytest", runtime: "python", install_methods: &[InstallMethod { method: "pip", package: "pytest" }] },
    ToolInfo { name: "a2x", runtime: "python", install_methods: &[InstallMethod { method: "apt", package: "asciidoc" }] },
    // The mako processor runs `python3 -c "import mako..."`, so what it really
    // needs is the Mako *library*, which the registry can't probe directly (it
    // probes executables). The `mako-render` console script ships in the same
    // distribution, so probing it is an exact proxy for "the library is
    // importable", and installing it installs the library.
    ToolInfo { name: "mako-render", runtime: "python", install_methods: &[InstallMethod { method: "pip", package: "mako" }] },
    ToolInfo { name: "python3", runtime: "python", install_methods: &[InstallMethod { method: "apt", package: "python3" }] },
    // Node tools
    ToolInfo { name: "marp", runtime: "node", install_methods: &[InstallMethod { method: "npm", package: "@marp-team/marp-cli" }] },
    ToolInfo { name: "mmdc", runtime: "node", install_methods: &[InstallMethod { method: "npm", package: "@mermaid-js/mermaid-cli" }] },
    ToolInfo { name: "node_modules/.bin/markdownlint", runtime: "node", install_methods: &[InstallMethod { method: "npm", package: "markdownlint-cli" }] },
    ToolInfo { name: "markdownlint", runtime: "node", install_methods: &[InstallMethod { method: "npm", package: "markdownlint-cli" }] },
    ToolInfo { name: "prettier", runtime: "node", install_methods: &[InstallMethod { method: "npm", package: "prettier" }] },
    ToolInfo { name: "eslint", runtime: "node", install_methods: &[InstallMethod { method: "npm", package: "eslint" }] },
    ToolInfo { name: "htmlhint", runtime: "node", install_methods: &[InstallMethod { method: "npm", package: "htmlhint" }] },
    ToolInfo { name: "jshint", runtime: "node", install_methods: &[InstallMethod { method: "npm", package: "jshint" }] },
    ToolInfo { name: "npm", runtime: "node", install_methods: &[InstallMethod { method: "apt", package: "npm" }] },
    ToolInfo { name: "node", runtime: "node", install_methods: &[InstallMethod { method: "apt", package: "nodejs" }] },
    // Ruby tools
    ToolInfo { name: "gems/bin/mdl", runtime: "ruby", install_methods: &[InstallMethod { method: "gem", package: "mdl" }] },
    ToolInfo { name: "mdl", runtime: "ruby", install_methods: &[InstallMethod { method: "gem", package: "mdl" }] },
    ToolInfo { name: "bundle", runtime: "ruby", install_methods: &[InstallMethod { method: "gem", package: "bundler" }] },
    ToolInfo { name: "ruby", runtime: "ruby", install_methods: &[InstallMethod { method: "apt", package: "ruby" }] },
    // Rust tools
    ToolInfo { name: "mdbook", runtime: "rust", install_methods: &[InstallMethod { method: "cargo", package: "mdbook" }] },
    ToolInfo { name: "rumdl", runtime: "rust", install_methods: &[
        InstallMethod { method: "binary", package: "rumdl" },
        InstallMethod { method: "cargo", package: "rumdl" },
    ]},
    ToolInfo { name: "taplo", runtime: "rust", install_methods: &[
        InstallMethod { method: "binary", package: "taplo" },
        InstallMethod { method: "cargo", package: "taplo-cli" },
    ]},
    // rustup is the canonical route, but `apt install rustc/cargo` is a real,
    // automatable install and is what makes these reachable from
    // `tools install --all` on a bare machine. A host that already has a
    // rustup toolchain never reaches the install path: `which` finds these
    // first.
    ToolInfo { name: "cargo", runtime: "rust", install_methods: &[InstallMethod { method: "apt", package: "cargo" }] },
    ToolInfo { name: "rustc", runtime: "rust", install_methods: &[InstallMethod { method: "apt", package: "rustc" }] },
    // Perl tools
    ToolInfo { name: "perl", runtime: "perl", install_methods: &[InstallMethod { method: "apt", package: "perl" }] },
    ToolInfo { name: "markdown", runtime: "perl", install_methods: &[InstallMethod { method: "apt", package: "markdown" }] },
    ToolInfo { name: "checkpatch.pl", runtime: "perl", install_methods: &[InstallMethod { method: "binary", package: "checkpatch.pl" }] },
    // System tools
    ToolInfo { name: "shellcheck", runtime: "system", install_methods: &[InstallMethod { method: "apt", package: "shellcheck" }] },
    ToolInfo { name: "luacheck", runtime: "system", install_methods: &[InstallMethod { method: "apt", package: "lua-check" }] },
    ToolInfo { name: "cppcheck", runtime: "system", install_methods: &[InstallMethod { method: "apt", package: "cppcheck" }] },
    ToolInfo { name: "clang-tidy", runtime: "system", install_methods: &[InstallMethod { method: "apt", package: "clang-tidy" }] },
    ToolInfo { name: "gcc", runtime: "system", install_methods: &[InstallMethod { method: "apt", package: "gcc" }] },
    ToolInfo { name: "g++", runtime: "system", install_methods: &[InstallMethod { method: "apt", package: "g++" }] },
    ToolInfo { name: "clang", runtime: "system", install_methods: &[InstallMethod { method: "apt", package: "clang" }] },
    ToolInfo { name: "clang++", runtime: "system", install_methods: &[InstallMethod { method: "apt", package: "clang" }] },
    ToolInfo { name: "ar", runtime: "system", install_methods: &[InstallMethod { method: "apt", package: "binutils" }] },
    ToolInfo { name: "make", runtime: "system", install_methods: &[InstallMethod { method: "apt", package: "make" }] },
    ToolInfo { name: "jq", runtime: "system", install_methods: &[InstallMethod { method: "apt", package: "jq" }] },
    ToolInfo { name: "aspell", runtime: "system", install_methods: &[InstallMethod { method: "apt", package: "aspell" }] },
    ToolInfo { name: "pandoc", runtime: "system", install_methods: &[InstallMethod { method: "apt", package: "pandoc" }] },
    ToolInfo { name: "pdflatex", runtime: "system", install_methods: &[InstallMethod { method: "apt", package: "texlive-latex-base" }] },
    ToolInfo { name: "qpdf", runtime: "system", install_methods: &[InstallMethod { method: "apt", package: "qpdf" }] },
    ToolInfo { name: "dot", runtime: "system", install_methods: &[InstallMethod { method: "apt", package: "graphviz" }] },
    ToolInfo { name: "drawio", runtime: "system", install_methods: &[InstallMethod { method: "snap", package: "drawio" }] },
    ToolInfo { name: "libreoffice", runtime: "system", install_methods: &[InstallMethod { method: "apt", package: "libreoffice" }] },
    ToolInfo { name: "flock", runtime: "system", install_methods: &[InstallMethod { method: "apt", package: "util-linux" }] },
    ToolInfo { name: "uname", runtime: "system", install_methods: &[InstallMethod { method: "apt", package: "coreutils" }] },
    ToolInfo { name: "sh", runtime: "system", install_methods: &[InstallMethod { method: "apt", package: "dash" }] },
    ToolInfo { name: "git", runtime: "system", install_methods: &[InstallMethod { method: "apt", package: "git" }] },
    ToolInfo { name: "pdfunite", runtime: "system", install_methods: &[InstallMethod { method: "apt", package: "poppler-utils" }] },
    ToolInfo { name: "google-chrome", runtime: "system", install_methods: &[InstallMethod { method: "apt", package: "google-chrome-stable" }] },
    ToolInfo { name: "objdump", runtime: "system", install_methods: &[InstallMethod { method: "apt", package: "binutils" }] },
    ToolInfo { name: "tidy", runtime: "system", install_methods: &[InstallMethod { method: "apt", package: "tidy" }] },
    ToolInfo { name: "xmllint", runtime: "system", install_methods: &[InstallMethod { method: "apt", package: "libxml2-utils" }] },
    ToolInfo { name: "clojure", runtime: "jvm", install_methods: &[
        // There is no apt/dnf package for the Clojure CLI. The official Linux
        // route is the install script (needs bash, curl, rlwrap); brew is the
        // macOS route. See https://clojure.org/guides/install_clojure
        InstallMethod { method: "manual", package: "run the official Linux installer (needs curl, bash, rlwrap): https://clojure.org/guides/install_clojure" },
        InstallMethod { method: "brew", package: "clojure/tools/clojure" },
    ]},
    ToolInfo { name: "svglint", runtime: "node", install_methods: &[InstallMethod { method: "npm", package: "svglint" }] },
    ToolInfo { name: "svgo", runtime: "node", install_methods: &[InstallMethod { method: "npm", package: "svgo" }] },
    ToolInfo { name: "cmakelint", runtime: "python", install_methods: &[InstallMethod { method: "pip", package: "cmakelint" }] },
    ToolInfo { name: "protoc", runtime: "system", install_methods: &[InstallMethod { method: "apt", package: "protobuf-compiler" }] },
    ToolInfo { name: "sass", runtime: "node", install_methods: &[InstallMethod { method: "npm", package: "sass" }] },
    ToolInfo { name: "hadolint", runtime: "system", install_methods: &[
        InstallMethod { method: "binary", package: "hadolint" },
        InstallMethod { method: "brew", package: "hadolint" },
        InstallMethod { method: "apt", package: "hadolint" },
    ]},
    ToolInfo { name: "php", runtime: "system", install_methods: &[InstallMethod { method: "apt", package: "php-cli" }] },
    ToolInfo { name: "checkstyle", runtime: "jvm", install_methods: &[InstallMethod { method: "apt", package: "checkstyle" }] },
    ToolInfo { name: "yq", runtime: "system", install_methods: &[
        InstallMethod { method: "pip", package: "yq" },
        InstallMethod { method: "snap", package: "yq" },
        InstallMethod { method: "apt", package: "yq" },
    ]},
    // Node tools (additional)
    ToolInfo { name: "stylelint", runtime: "node", install_methods: &[InstallMethod { method: "npm", package: "stylelint" }] },
    ToolInfo { name: "jslint", runtime: "node", install_methods: &[InstallMethod { method: "npm", package: "jslint" }] },
    ToolInfo { name: "standard", runtime: "node", install_methods: &[InstallMethod { method: "npm", package: "standard" }] },
    ToolInfo { name: "htmllint", runtime: "node", install_methods: &[InstallMethod { method: "npm", package: "htmllint-cli" }] },
    ToolInfo { name: "slidev", runtime: "node", install_methods: &[InstallMethod { method: "npm", package: "@slidev/cli" }] },
    // Perl tools (additional)
    ToolInfo { name: "perlcritic", runtime: "perl", install_methods: &[InstallMethod { method: "apt", package: "libperl-critic-perl" }] },
    // Ruby tools (additional)
    ToolInfo { name: "jekyll", runtime: "ruby", install_methods: &[InstallMethod { method: "gem", package: "jekyll" }] },
    // Built-in / coreutils
    ToolInfo { name: "true", runtime: "system", install_methods: &[InstallMethod { method: "apt", package: "coreutils" }] },
];

/// Look up a tool by name.
pub fn tool_info(tool: &str) -> Option<&'static ToolInfo> {
    TOOLS.iter().find(|t| t.name == tool)
}

/// Return the default install command for a tool, if known.
pub fn tool_install_command(tool: &str) -> Option<String> {
    tool_info(tool).and_then(|t| t.install_methods.first().map(InstallMethod::command))
}

/// Return the runtime category for a tool, if known.
pub fn tool_runtime(tool: &str) -> Option<&'static str> {
    tool_info(tool).map(|t| t.runtime)
}

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

/// A single failed product with structured error info for `rsconstruct edit`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FailedProduct {
    /// Primary input file path
    pub file: String,
    /// Processor (instance) name
    pub processor: String,
    /// Error message from the tool
    pub error: String,
}

/// Aggregated statistics from all processors
#[derive(Default)]
pub struct BuildStats {
    pub categories: Vec<ProcessStats>,
    pub total_duration: Duration,
    pub failed_count: usize,
    pub failed_messages: Vec<String>,
    /// Structured failure details for `rsconstruct edit`
    pub failed_details: Vec<FailedProduct>,
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

// ----------------------------------------------------------------------------
// Shared runtime types for data-driven per-processor files.
//
// Most single-file processors don't need their own Processor struct — they
// configure one of these generic runtimes instead and submit a plugin entry.
// Moved here from checkers/simple.rs and generators/simple.rs so the
// checkers/ and generators/ directories contain ONLY per-processor files.
// ----------------------------------------------------------------------------

/// A simple checker processor driven entirely by data.
/// Each trivial checker file (ruff.rs, pylint.rs, etc.) registers an instance
/// of this struct with its own `SimpleCheckerParams`.
pub struct SimpleChecker {
    config: CheckerConfigWithCommand,
    params: SimpleCheckerParams,
}

impl SimpleChecker {
    pub const fn new(config: CheckerConfigWithCommand, params: SimpleCheckerParams) -> Self {
        Self { config, params }
    }

    fn check_files(&self, ctx: &crate::build_context::BuildContext, files: &[&Path]) -> Result<()> {
        let tool = self.config.standard.require_command(self.params.description)?;
        if self.params.prepend_args.is_empty() {
            run_checker(ctx, tool, self.params.subcommand, &self.config.standard.args, files, ctx.max_arg_len())
        } else {
            let mut combined_args: Vec<String> = self.params.prepend_args.iter().map(std::string::ToString::to_string).collect();
            combined_args.extend_from_slice(&self.config.standard.args);
            run_checker(ctx, tool, self.params.subcommand, &combined_args, files, ctx.max_arg_len())
        }
    }

    const fn has_fix(&self) -> bool {
        self.params.fix_subcommand.is_some() || !self.params.fix_prepend_args.is_empty()
    }

    fn fix_files(&self, ctx: &crate::build_context::BuildContext, files: &[&Path]) -> Result<()> {
        let tool = self.config.standard.require_command(self.params.description)?;
        let subcommand = self.params.fix_subcommand.or(self.params.subcommand);
        if self.params.fix_prepend_args.is_empty() {
            run_checker(ctx, tool, subcommand, &self.config.standard.args, files, ctx.max_arg_len())
        } else {
            let mut combined_args: Vec<String> = self.params.fix_prepend_args.iter().map(std::string::ToString::to_string).collect();
            combined_args.extend_from_slice(&self.config.standard.args);
            run_checker(ctx, tool, subcommand, &combined_args, files, ctx.max_arg_len())
        }
    }
}

impl Processor for SimpleChecker {
    fn scan_config(&self) -> &StandardConfig {
        &self.config.standard
    }

    fn auto_detect(&self, file_index: &FileIndex) -> bool {
        !file_index.scan(&self.config.standard, true).is_empty()
    }

    fn required_tools(&self) -> Vec<String> {
        let mut tools = vec![self.config.standard.command.clone()];
        for t in self.params.extra_tools {
            tools.push(t.to_string());
        }
        tools
    }

    fn discover(
        &self,
        graph: &mut BuildGraph,
        file_index: &FileIndex,
        instance_name: &str,
    ) -> Result<()> {
        discover_checker_products(
            graph, &self.config.standard, file_index,
            &self.config.standard.dep_inputs, &self.config.standard.dep_auto,
            &self.config,
            <crate::config::CheckerConfigWithCommand as crate::config::KnownFields>::checksum_fields(),
            instance_name,
        )
    }

    fn execute(&self, ctx: &crate::build_context::BuildContext, product: &Product) -> Result<()> {
        self.check_files(ctx, &[product.primary_input()])
    }

    fn execute_batch(&self, ctx: &crate::build_context::BuildContext, products: &[&Product]) -> Vec<Result<()>> {
        execute_checker_batch(ctx, products, |ctx, files| self.check_files(ctx, files))
    }

    fn fix(&self, ctx: &crate::build_context::BuildContext, product: &Product) -> Result<()> {
        self.fix_files(ctx, &[product.primary_input()])
    }

    fn supports_fix_batch(&self) -> bool {
        self.has_fix() && self.params.fix_batch.unwrap_or(self.config.standard.batch)
    }

    fn fix_batch(&self, ctx: &crate::build_context::BuildContext, products: &[&Product]) -> Vec<Result<()>> {
        execute_checker_batch(ctx, products, |ctx, files| self.fix_files(ctx, files))
    }
}

/// How a simple generator discovers its products.
#[derive(Copy, Clone)]
pub enum DiscoverMode {
    /// Discover one product per source x format (uses config.formats).
    MultiFormat,
    /// Discover one product per source file with a fixed output extension.
    SingleFormat(&'static str),
}

/// Parameters for a [`SimpleGenerator`]. Each trivial generator file
/// (mermaid.rs, pandoc.rs, etc.) configures one and registers it via the
/// processor registry.
#[derive(Copy, Clone)]
pub struct SimpleGeneratorParams {
    pub extra_tools: &'static [&'static str],
    pub discover_mode: DiscoverMode,
    pub execute_fn: fn(&crate::build_context::BuildContext, &StandardConfig, &Product) -> Result<()>,
    pub is_native: bool,
}

/// Data-driven generator processor. Replaces identical boilerplate across
/// generators that use `StandardConfig` with standard discover logic.
pub struct SimpleGenerator {
    config: StandardConfig,
    params: SimpleGeneratorParams,
}

impl SimpleGenerator {
    pub const fn new(config: StandardConfig, params: SimpleGeneratorParams) -> Self {
        Self {
            config,
            params,
        }
    }
}

impl Processor for SimpleGenerator {
    fn scan_config(&self) -> &StandardConfig {
        &self.config
    }

    fn config_json(&self) -> Option<String> {
        ProcessorBase::config_json(&self.config)
    }

    fn clean(&self, product: &Product, verbose: bool) -> Result<usize> {
        ProcessorBase::clean(product, &product.processor, verbose)
    }

    fn required_tools(&self) -> Vec<String> {
        if self.params.is_native {
            self.params.extra_tools.iter().map(std::string::ToString::to_string).collect()
        } else {
            let mut tools = vec![self.config.command.clone()];
            for t in self.params.extra_tools {
                tools.push(t.to_string());
            }
            tools
        }
    }

    fn discover(&self, graph: &mut BuildGraph, file_index: &FileIndex, instance_name: &str) -> Result<()> {
        let params = generators::DiscoverParams {
            scan: &self.config,
            dep_inputs: &self.config.dep_inputs,
            config: &self.config,
            output_dir: &self.config.output_dir,
            processor_name: instance_name,
            checksum_fields: <crate::config::StandardConfig as crate::config::KnownFields>::checksum_fields(),
        };
        match &self.params.discover_mode {
            DiscoverMode::MultiFormat => {
                generators::discover_multi_format(graph, file_index, &params, &self.config.formats)
            }
            DiscoverMode::SingleFormat(ext) => {
                generators::discover_single_format(graph, file_index, &params, ext)
            }
        }
    }

    fn execute(&self, ctx: &crate::build_context::BuildContext, product: &Product) -> Result<()> {
        (self.params.execute_fn)(ctx, &self.config, product)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::create_all_default_processors;

    /// Verify that every tool declared by any processor's `required_tools()` has
    /// an entry in the central TOOLS registry (install command + runtime category).
    /// This prevents silent gaps like the missing `ar` tool.
    #[test]
    fn all_required_tools_have_registry_entries() {
        let processors = create_all_default_processors().unwrap();
        for (proc_name, proc) in &processors {
            for tool in proc.required_tools() {
                if tool.is_empty() {
                    continue;
                }
                assert!(
                    tool_install_command(&tool).is_some(),
                    "Processor '{proc_name}' requires tool '{tool}' which has no install command in TOOLS"
                );
                assert!(
                    tool_runtime(&tool).is_some(),
                    "Processor '{proc_name}' requires tool '{tool}' which has no runtime category in TOOLS"
                );
            }
        }
    }

    /// `is_native` is declared twice for every `SimpleGenerator`: once in
    /// `SimpleGeneratorParams` (where it decides whether `config.command`
    /// counts as a required tool) and once in the plugin registration (where
    /// it drives `processors list`). Nothing linked them, so the two could
    /// silently disagree — a processor could be listed as native while still
    /// demanding an external tool, or vice versa.
    ///
    /// `SimpleGenerator` declares `is_native` twice — once in
    /// `SimpleGeneratorParams` (where it makes `required_tools()` drop
    /// `config.command`) and once in the plugin registration (where it
    /// drives `processors list`). Nothing linked them, so flipping one alone
    /// left a processor listed as native while still demanding a binary, or
    /// the reverse.
    ///
    /// The two are reconciled here through observable behavior: for a
    /// `SimpleGenerator` the params flag is the *only* thing that decides
    /// whether the default `command` appears in `required_tools()`, so
    /// comparing that against the registry flag pins them together.
    ///
    /// Scoped to `SimpleGenerator` deliberately. Hand-written processors have
    /// legitimate reasons to be native yet name tools (tera renders
    /// in-process but its template functions can shell out to `git`), and
    /// non-native ones may run something other than their configured command
    /// (clippy runs `cargo`).
    #[test]
    fn simple_generator_is_native_declarations_agree() {
        // Each entry: (processor name, the params flag it was constructed
        // with). Kept next to the assertion rather than derived, because
        // SimpleGenerator does not know its own name at runtime — which is
        // the root cause of the duplication in the first place.
        let simple_generators: &[(&str, bool)] = &[
            ("yaml2json", true),
            ("imarkdown2html", true),
            ("isass", true),
            ("markdown2html", false),
            ("sass", false),
            ("a2x", false),
            ("chromium", false),
            ("protobuf", false),
            ("objdump", false),
            ("mermaid", false),
            ("libreoffice", false),
            ("drawio", false),
        ];

        for (name, params_native) in simple_generators {
            let registry_native = crate::registries::is_native(name);
            assert_eq!(
                *params_native, registry_native,
                "Processor '{name}': SimpleGeneratorParams says is_native={params_native} \
                 but the plugin registration says is_native={registry_native}. These are \
                 two hand-maintained copies of one fact; update both.",
            );
        }
    }

    /// `sudo_argv()` returns either `["sudo"]` or `[]` based on runtime
    /// state. Empty when running as root or when `sudo` isn't on PATH.
    #[test]
    fn sudo_argv_matches_runtime() {
        let prefix = sudo_argv();
        if crate::platform::needs_sudo() {
            assert_eq!(prefix, &["sudo"]);
        } else {
            assert!(prefix.is_empty());
        }
    }

    /// `apt` describe must run `apt-get update` first, then call
    /// `apt-get install -y <pkg>` exactly once with the package name
    /// after the flags. Whether `sudo` is prepended depends on runtime
    /// state.
    #[test]
    fn apt_describe_shape_is_correct() {
        let steps = describe("apt", &["cowsay"]);
        assert_eq!(steps.len(), 2);

        let update = &steps[0];
        let upd_idx = update.iter().position(|s| s == "apt-get").expect("apt-get in update argv");
        assert_eq!(update[upd_idx + 1], "update");
        assert_eq!(update.len(), upd_idx + 2);

        let install = &steps[1];
        let pkgmgr_idx = install.iter().position(|s| s == "apt-get").expect("apt-get in install argv");
        assert_eq!(install[pkgmgr_idx + 1], "install");
        assert_eq!(install[pkgmgr_idx + 2], "-y");
        assert_eq!(install[pkgmgr_idx + 3], "cowsay");
        if install[0] == "sudo" {
            assert_eq!(pkgmgr_idx, 1);
        } else {
            assert_eq!(pkgmgr_idx, 0);
        }
    }

    /// `apt` batch describe collapses many packages into one apt-get
    /// install call (still preceded by a single `apt-get update`).
    #[test]
    fn apt_batch_describe_collapses() {
        let steps = describe("apt", &["foo", "bar", "baz"]);
        assert_eq!(steps.len(), 2);
        let install = &steps[1];
        let pkgmgr_idx = install.iter().position(|s| s == "apt-get").expect("apt-get in argv");
        assert_eq!(&install[pkgmgr_idx + 3..], &["foo", "bar", "baz"]);
    }

    /// `pip` describe never uses sudo, regardless of runtime state.
    #[test]
    fn pip_describe_never_has_sudo() {
        let steps = describe("pip", &["ruff"]);
        assert_eq!(steps.len(), 1);
        let argv = &steps[0];
        assert_eq!(argv[0], "pip");
        assert!(!argv.contains(&"sudo".to_string()));
    }

    /// `binary` describe yields a multi-step plan (download, extract,
    /// chmod, mv) with no shell metacharacters anywhere.
    #[test]
    fn binary_describe_has_no_shell_metachars() {
        for pkg in &["taplo", "rumdl"] {
            let steps = describe("binary", &[pkg]);
            assert!(steps.len() >= 3, "binary {pkg} should have >=3 steps");
            for step in &steps {
                for arg in step {
                    for forbidden in &['|', '>', '<', ';', '&'] {
                        assert!(
                            !arg.contains(*forbidden),
                            "binary {pkg} step contains shell metachar '{forbidden}': {arg:?}"
                        );
                    }
                }
            }
        }
    }

    /// Chunks must respect the limit, and their concatenation must be
    /// exactly the input — no file dropped, none duplicated.
    #[test]
    fn checker_chunks_respect_limit_and_cover_all_files() {
        let bufs: Vec<PathBuf> = (0..6).map(|i| PathBuf::from(format!("fil{i}"))).collect();
        let files: Vec<&Path> = bufs.iter().map(PathBuf::as_path).collect();
        // base 10 + two files of cost 5 each = 20 exactly; a third would overflow.
        let ranges = checker_chunk_ranges(10, &files, 20);

        assert_eq!(ranges, vec![(0, 2), (2, 4), (4, 6)]);
        for &(start, end) in &ranges {
            let cost: usize = 10 + files[start..end].iter()
                .map(|f| f.as_os_str().len() + 1).sum::<usize>();
            assert!(cost <= 20, "chunk {start}..{end} costs {cost}");
        }
    }

    /// A single path longer than the limit must still get its own
    /// (over-limit) chunk — the alternative is an infinite loop.
    #[test]
    fn checker_chunks_oversized_path_still_makes_progress() {
        let long = PathBuf::from("x".repeat(50));
        let small = PathBuf::from("ok");
        let files: Vec<&Path> = vec![long.as_path(), small.as_path()];
        let ranges = checker_chunk_ranges(5, &files, 20);
        assert_eq!(ranges, vec![(0, 1), (1, 2)],
            "oversized path alone, then the rest");
    }

    /// Every chunk re-pays the base command length — with a base that
    /// nearly fills the limit, each chunk holds exactly one file.
    #[test]
    fn checker_chunks_repay_base_len_per_chunk() {
        let bufs: Vec<PathBuf> = (0..4).map(|i| PathBuf::from(format!("{i}"))).collect();
        let files: Vec<&Path> = bufs.iter().map(PathBuf::as_path).collect();
        // base 18 + one file of cost 2 = 20; a second file would need 22.
        let ranges = checker_chunk_ranges(18, &files, 20);
        assert_eq!(ranges.len(), files.len(),
            "base_len must be budgeted in every chunk, not only the first");
    }

    /// External-tool batches have one exit status for the whole chunk, so a
    /// failure must fan out to every product in it — and a success must not.
    #[test]
    fn checker_batch_failure_fans_out_to_all_products() {
        let ctx = crate::build_context::BuildContext::new();
        let mut g = crate::graph::BuildGraph::new();
        for name in ["a.py", "b.py", "c.py"] {
            g.add_product(vec![PathBuf::from(name)], vec![], "check", None).unwrap();
        }
        let products: Vec<&crate::graph::Product> = g.products().iter().collect();

        let failed = execute_checker_batch(&ctx, &products, |_, files| {
            assert_eq!(files.len(), 3, "tool must see the whole chunk");
            anyhow::bail!("tool reported problems")
        });
        assert_eq!(failed.len(), 3);
        for r in &failed {
            let msg = r.as_ref().unwrap_err().to_string();
            assert!(msg.contains("tool reported problems"), "got: {msg}");
        }

        let passed = execute_checker_batch(&ctx, &products, |_, _| Ok(()));
        assert!(passed.iter().all(Result::is_ok));
    }

    /// In-process checkers report per file: one bad file must fail only its
    /// own product — this is what --keep-going correctness rests on.
    #[test]
    fn checker_batch_per_file_fails_only_its_own_product() {
        let mut g = crate::graph::BuildGraph::new();
        for name in ["good1.py", "bad.py", "good2.py"] {
            g.add_product(vec![PathBuf::from(name)], vec![], "check", None).unwrap();
        }
        let products: Vec<&crate::graph::Product> = g.products().iter().collect();

        let results = execute_checker_batch_per_file(&products, |path| {
            anyhow::ensure!(!path.ends_with("bad.py"), "bad file");
            Ok(())
        });
        assert!(results[0].is_ok());
        assert!(results[1].is_err());
        assert!(results[2].is_ok());
    }

    /// `run_command_with_stdin` must survive a child that writes far more
    /// than a pipe buffer's worth of output while we are still feeding it.
    ///
    /// This is the deadlock the hand-rolled aspell spawn worked around with
    /// a writer thread: feeding all of stdin before draining stdout wedges
    /// as soon as the child fills its output pipe. 1 MB is well past the
    /// typical 64 KB pipe buffer in both directions.
    #[cfg(unix)]
    #[test]
    fn stdin_and_output_are_pumped_concurrently() {
        crate::runtime_flags::init_for_test();
        let ctx = crate::build_context::BuildContext::new();
        let payload = "x".repeat(1024 * 1024);

        let mut cmd = Command::new("cat");
        let output = run_command_with_stdin(&ctx, &cmd, payload.as_bytes()).unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout.len(), payload.len(),
            "cat must echo every byte we wrote");

        // A child that ignores stdin entirely must not hang or error: it
        // closes the pipe early (EPIPE on our side), which is normal.
        cmd = Command::new("true");
        let output = run_command_with_stdin(&ctx, &cmd, payload.as_bytes()).unwrap();
        assert!(output.status.success());
    }

    /// A stdin write failure is reported only when the child also failed —
    /// where truncated input is a plausible cause. A successful child that
    /// simply stopped reading is not an error.
    #[cfg(unix)]
    #[test]
    fn failing_child_that_ignored_stdin_reports_its_own_failure() {
        crate::runtime_flags::init_for_test();
        let ctx = crate::build_context::BuildContext::new();
        let payload = "y".repeat(1024 * 1024);

        let cmd = Command::new("false");
        let result = run_command_with_stdin(&ctx, &cmd, payload.as_bytes());
        // Either an Err (stdin write surfaced) or a non-zero exit is
        // acceptable; a hang or a spurious Ok(success) is not.
        if let Ok(output) = result {
            assert!(!output.status.success());
        }
    }
}
