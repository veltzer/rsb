mod checkers;
mod explicit;
pub mod generators;
mod creators;
pub mod lua;

use anyhow::{Context, Result};
use serde::Serialize;
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
// Used by the check in run_command_inner() to catch undeclared tool usage.
//
// One always-compiled implementation, no `#[cfg]` fork: the previous
// `#[cfg(not(debug_assertions))]` no-op stubs were dead code `cargo test`
// could never compile, which is the exact failure mode the project's no-cfg
// rule names. The bookkeeping is a thread_local swap per product execution —
// noise next to a process spawn. Only the *panic* stays debug-gated (via
// `debug_assert!` at the check site).
thread_local! {
    static DECLARED_TOOLS: RefCell<Option<Vec<String>>> = const { RefCell::new(None) };
}

/// Set the declared tools for the current thread.
pub fn set_declared_tools(tools: Option<Vec<String>>) {
    DECLARED_TOOLS.with(|dt| {
        *dt.borrow_mut() = tools;
    });
}

/// Temporarily suspend the declared-tools check for user-specified commands.
/// Returns a guard that restores the previous value when dropped.
pub fn suspend_tool_check() -> ToolCheckGuard {
    let prev = DECLARED_TOOLS.with(|dt| dt.borrow_mut().take());
    ToolCheckGuard { prev }
}

/// RAII guard that restores the declared tools when dropped.
pub struct ToolCheckGuard {
    prev: Option<Vec<String>>,
}

impl Drop for ToolCheckGuard {
    fn drop(&mut self) {
        DECLARED_TOOLS.with(|dt| {
            *dt.borrow_mut() = self.prev.take();
        });
    }
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

    DECLARED_TOOLS.with(|dt| {
        if let Some(ref tools) = *dt.borrow() {
            let program = cmd.get_program().to_string_lossy();
            let basename = program.rsplit('/').next().unwrap_or(&program);
            debug_assert!(
                tools.iter().any(|t| {
                    let t_basename = t.rsplit('/').next().unwrap_or(t);
                    t_basename == basename
                }),
                "Processor executed undeclared tool '{basename}'. Declared: {tools:?}",
            );
        }
    });

    if ctx.is_interrupted() {
        return Err(crate::exit_code::interrupted());
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
            return Err(crate::exit_code::interrupted());
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
                Err(crate::exit_code::interrupted())
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
                Err(crate::exit_code::interrupted())
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
    // Dedupe against the file's CURRENT contents, not just the caller's
    // snapshot: another instance (or an earlier flush in a watch session)
    // may have appended since the snapshot was taken at construction, and
    // append-only writing would silently accumulate duplicates.
    let mut known: HashSet<String> = existing.clone();
    match std::fs::read_to_string(words_path) {
        Ok(content) => {
            known.extend(content.lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_string));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(e).with_context(|| format!("Failed to read words file: {}", words_path.display()));
        }
    }
    let to_add: Vec<_> = new_words.iter()
        .filter(|w| !known.contains(*w))
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
    /// Allowlist of field names to include in the config-change checksum,
    /// derived from the plugin's `FieldSpec` list (`checksum_fields_of`).
    pub checksum_fields: Vec<&'static str>,
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

    let hash = Some(output_config_hash(cfg_hash, &checksum_fields));
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
    /// (A `processor_type()` Lua hook was once documented as overriding this;
    /// no Rust code ever read it — all Lua plugins are categorized as Lua.)
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
/// The processor type, name, and description live in the plugin
/// registration (`inventory::submit!` of a `ProcessorPlugin`), not on this
/// trait — metadata must be available without instantiating a processor.
///
/// # Implementing a Checker
///
/// Checkers are simpler - just implement the required methods and use defaults for the rest:
///
/// ```ignore
/// impl Processor for MyChecker {
///     fn scan_config(&self) -> &StandardConfig { &self.config.standard }
///     fn execute(&self, ctx: &BuildContext, product: &Product) -> Result<()> {
///         run_mytool(ctx, product.primary_input())
///     }
/// }
/// ```
///
/// # Implementing a Generator
///
/// Generators must also declare outputs in `discover()` and override `clean()`:
///
/// ```ignore
/// impl Processor for MyGenerator {
///     fn scan_config(&self) -> &StandardConfig { &self.config.standard }
///     fn discover(&self, graph: &mut BuildGraph, file_index: &FileIndex, instance_name: &str) -> Result<()> {
///         graph.add_product(inputs, outputs, instance_name, ...)?;  // non-empty outputs
///     }
///     fn execute(&self, ctx: &BuildContext, product: &Product) -> Result<()> { ... }
///     fn clean(&self, product: &Product, verbose: bool) -> Result<usize> {
///         clean_outputs(product, &product.processor, verbose)
///     }
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
        // User-declared extras, for a `command` that is a wrapper script.
        tools.extend(self.config.standard.required_tools.iter().cloned());
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
/// Parameters for a [`SimpleGenerator`] over config type `C`.
///
/// `execute_fn` receives the whole `&C`, not just its `StandardConfig`, which
/// is what lets a generator with extra config fields (pandoc's `pdf_engine`,
/// say) use `SimpleGenerator` instead of hand-rolling the whole trait.
/// `extra_tools_fn` covers the other reason generators used to be hand-rolled:
/// a required tool named by a config field rather than a constant.
pub struct SimpleGeneratorParams<C> {
    pub extra_tools: &'static [&'static str],
    /// Additional required tools derived from the config (e.g. pandoc's
    /// `pdf_engine`). `None` means the static `extra_tools` are the whole set.
    pub extra_tools_fn: Option<fn(&C) -> Vec<String>>,
    pub discover_mode: DiscoverMode,
    pub execute_fn: fn(&crate::build_context::BuildContext, &C, &Product) -> Result<()>,
    pub is_native: bool,
}

// Manual Copy/Clone: `#[derive]` would demand `C: Copy`/`C: Clone`, but every
// field here is a fn pointer or plain data — the config type is only named in
// their signatures, never stored.
impl<C> Clone for SimpleGeneratorParams<C> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<C> Copy for SimpleGeneratorParams<C> {}

/// Data-driven generator processor. Replaces identical boilerplate across
/// generators with standard discover logic.
///
/// Generic over the config type so a generator needing one extra field does
/// not have to reimplement `Processor`. `C: AsRef<StandardConfig>` supplies
/// the scan/discover half; `KnownFields` supplies the checksum allowlist.
pub struct SimpleGenerator<C> {
    config: C,
    params: SimpleGeneratorParams<C>,
}

impl<C> SimpleGenerator<C> {
    pub const fn new(config: C, params: SimpleGeneratorParams<C>) -> Self {
        Self {
            config,
            params,
        }
    }
}

impl<C> Processor for SimpleGenerator<C>
where
    C: AsRef<StandardConfig> + serde::Serialize + Send + Sync,
{
    fn scan_config(&self) -> &StandardConfig {
        self.config.as_ref()
    }

    fn config_json(&self) -> Option<String> {
        ProcessorBase::config_json(&self.config)
    }

    fn clean(&self, product: &Product, verbose: bool) -> Result<usize> {
        ProcessorBase::clean(product, &product.processor, verbose)
    }

    fn required_tools(&self) -> Vec<String> {
        let mut tools = if self.params.is_native {
            Vec::new()
        } else {
            vec![self.config.as_ref().command.clone()]
        };
        for t in self.params.extra_tools {
            tools.push(t.to_string());
        }
        if let Some(f) = self.params.extra_tools_fn {
            tools.extend(f(&self.config));
        }
        // User-declared extras, for a `command` that is a wrapper script.
        tools.extend(self.config.as_ref().required_tools.iter().cloned());
        tools
    }

    fn discover(&self, graph: &mut BuildGraph, file_index: &FileIndex, instance_name: &str) -> Result<()> {
        let scan = self.config.as_ref();
        let params = generators::DiscoverParams {
            scan,
            dep_inputs: &scan.dep_inputs,
            config: &self.config,
            output_dir: &scan.output_dir,
            processor_name: instance_name,
            checksum_fields: crate::config::checksum_fields_of(instance_name),
        };
        match &self.params.discover_mode {
            DiscoverMode::MultiFormat => {
                generators::discover_multi_format(graph, file_index, &params, &scan.formats)
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
                    crate::tools::tool_install_command(&tool).is_some(),
                    "Processor '{proc_name}' requires tool '{tool}' which has no install command in TOOLS"
                );
                assert!(
                    crate::tools::tool_runtime(&tool).is_some(),
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
            ("pandoc", false),
        ];

        // Completeness: the list above is a hand-maintained copy, and it
        // silently skipped pandoc once. Pin it against the actual number of
        // SimpleGeneratorParams construction sites in the generators tree
        // (source-scanning at test time, same technique as the bare-println
        // scanner) so a new SimpleGenerator that skips this list fails here.
        let mut param_sites = 0;
        for entry in std::fs::read_dir("src/processors/generators").unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|e| e == "rs") {
                let src = std::fs::read_to_string(&path).unwrap();
                param_sites += src.matches("SimpleGeneratorParams {").count();
            }
        }
        assert_eq!(simple_generators.len(), param_sites,
            "SimpleGenerator construction sites and this list disagree — a \
             new SimpleGenerator must be added here so its is_native \
             declarations stay pinned");

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
