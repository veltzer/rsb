use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::process::Command;
use parking_lot::Mutex;

use crate::processors::{check_command_output, run_command_capture};

/// Global cache for shell command results to avoid running the same command multiple times.
/// Key is the command string, value is the resulting flags.
/// Thread-safe via `parking_lot::Mutex`. Lives for the process lifetime (never cleared).
static SHELL_COMMAND_CACHE: Mutex<Option<HashMap<String, Vec<String>>>> = Mutex::new(None);

/// Get or compute flags from a shell command, caching the result.
fn cached_shell_command(cmd_line: &str, runner: impl FnOnce(&crate::build_context::BuildContext, &str) -> Result<Vec<String>>, ctx: &crate::build_context::BuildContext) -> Result<Vec<String>> {
    let mut guard = SHELL_COMMAND_CACHE.lock();
    let cache = guard.get_or_insert_with(HashMap::new);

    if let Some(cached) = cache.get(cmd_line) {
        return Ok(cached.clone());
    }

    let result = runner(ctx, cmd_line)?;
    cache.insert(cmd_line.to_string(), result.clone());
    Ok(result)
}

/// Per-file compile/link flags extracted from source comments.
#[derive(Default)]
struct SourceFlags {
    compile_args_before: Vec<String>,
    compile_args_after: Vec<String>,
    link_args_before: Vec<String>,
    link_args_after: Vec<String>,
}

/// Extract the comment value from a C/C++ source line.
///
/// Handles three comment styles:
///   - `// ...` — returns the text after `//`
///   - `/* ... */` — returns the text between `/*` and `*/`
///   - `* ...` — block comment continuation line, returns the text after `*`
///
/// Returns None for non-comment lines or empty/closing block comment lines.
fn extract_comment_value(line: &str) -> Option<&str> {
    let trimmed = line.trim();

    // Try // comment style
    if let Some(rest) = trimmed.strip_prefix("//") {
        return Some(rest.trim());
    }
    // Try /* ... */ comment style (single-line)
    if let Some(rest) = trimmed.strip_prefix("/*") {
        return rest.strip_suffix("*/").map(str::trim);
    }
    // Try block comment continuation line: * ...
    if let Some(rest) = trimmed.strip_prefix('*') {
        let rest = rest.trim();
        if rest.is_empty() || rest == "/" {
            return None;
        }
        return Some(rest.strip_suffix("*/").map(str::trim).unwrap_or(rest));
    }
    None
}

/// Check if a source file should be excluded from a specific compiler profile.
///
/// Looks for directives like:
///   // `EXCLUDE_PROFILE=clang`
///   // `EXCLUDE_PROFILE=gcc` clang
///
/// Returns true if the file should be excluded for the given profile.
fn should_exclude_for_profile(source: &Path, profile_name: &str) -> bool {
    // If profile name is empty (single compiler setup), never exclude
    if profile_name.is_empty() {
        return false;
    }

    let Ok(content) = fs::read_to_string(source) else { return false };

    for line in content.lines() {
        let Some(value_part) = extract_comment_value(line) else {
            continue;
        };

        if let Some(rest) = value_part.strip_prefix("EXCLUDE_PROFILE")
            && let Some(profiles_str) = rest.strip_prefix('=')
            && profiles_str.split_whitespace().any(|p| p == profile_name) {
                return true;
            }
    }

    false
}

/// Parse per-file flags from C/C++ source comment lines.
///
/// Supported comment formats:
///   // EXTRA_COMPILE_FLAGS_BEFORE=-pthread -I/usr/local/include
///   /* EXTRA_COMPILE_FLAGS_AFTER=-O2 -DNDEBUG */
///   // EXTRA_COMPILE_CMD=pkg-config --cflags ACE
///   // EXTRA_LINK_CMD=pkg-config --libs ACE
///   // `EXTRA_COMPILE_SHELL=echo` -`DLEVEL2_CACHE_LINESIZE=$(getconf` `LEVEL2_CACHE_LINESIZE`)
///   // `EXTRA_LINK_SHELL=echo` -L$(brew --prefix openssl)/lib
///
/// Compiler profile-specific flags (only applied when compiling with the named profile):
///   // `EXTRA_COMPILE_FLAGS_BEFORE`[gcc]=-femit-struct-debug-baseonly
///   // `EXTRA_COMPILE_FLAGS_BEFORE`[clang]=-gline-tables-only
///
/// Exclude file from specific profiles:
///   // `EXCLUDE_PROFILE=clang`
///   // `EXCLUDE_PROFILE=gcc` clang
///
/// `EXTRA_*_FLAGS_*` values are literal flags (with backtick expansion).
/// `EXTRA_*_CMD` values are executed as a subprocess (no shell) and stdout is used as flags.
/// `EXTRA_*_SHELL` values are executed via `sh -c` and stdout is used as flags.
///
/// The `profile_name` parameter specifies the current compiler profile name.
/// Directives without a profile suffix apply to all profiles.
/// Directives with a profile suffix (e.g., `[gcc]`) only apply when that profile is active.
fn parse_source_flags(ctx: &crate::build_context::BuildContext, source: &Path, profile_name: &str) -> Result<SourceFlags> {
    let content = fs::read_to_string(source)
        .with_context(|| format!("Failed to read source file: {}", source.display()))?;

    let mut flags = SourceFlags::default();

    let args_var_names = [
        "EXTRA_COMPILE_FLAGS_BEFORE",
        "EXTRA_COMPILE_FLAGS_AFTER",
        "EXTRA_LINK_FLAGS_BEFORE",
        "EXTRA_LINK_FLAGS_AFTER",
    ];

    let cmd_var_names = [
        "EXTRA_COMPILE_CMD",
        "EXTRA_LINK_CMD",
    ];

    let shell_var_names = [
        "EXTRA_COMPILE_SHELL",
        "EXTRA_LINK_SHELL",
    ];

    for line in content.lines() {
        let Some(value_part) = extract_comment_value(line) else {
            continue;
        };

        for var_name in &args_var_names {
            if let Some((rest, applies)) = match_directive_with_profile(value_part, var_name, profile_name)
                && applies
                    && let Some(raw_value) = rest.strip_prefix('=') {
                        let expanded = expand_backticks(ctx, raw_value.trim())?;
                        let args: Vec<String> = expanded
                            .split_whitespace()
                            .map(String::from)
                            .collect();
                        match *var_name {
                            "EXTRA_COMPILE_FLAGS_BEFORE" => flags.compile_args_before.extend(args),
                            "EXTRA_COMPILE_FLAGS_AFTER" => flags.compile_args_after.extend(args),
                            "EXTRA_LINK_FLAGS_BEFORE" => flags.link_args_before.extend(args),
                            "EXTRA_LINK_FLAGS_AFTER" => flags.link_args_after.extend(args),
                            _ => {}
                        }
                    }
        }

        for var_name in &cmd_var_names {
            if let Some((rest, applies)) = match_directive_with_profile(value_part, var_name, profile_name)
                && applies
                    && let Some(raw_value) = rest.strip_prefix('=') {
                        let cmd = raw_value.trim();
                        let args = cached_shell_command(cmd, run_command_for_flags, ctx)?;
                        match *var_name {
                            "EXTRA_COMPILE_CMD" => flags.compile_args_after.extend(args),
                            "EXTRA_LINK_CMD" => flags.link_args_after.extend(args),
                            _ => {}
                        }
                    }
        }

        for var_name in &shell_var_names {
            if let Some((rest, applies)) = match_directive_with_profile(value_part, var_name, profile_name)
                && applies
                    && let Some(raw_value) = rest.strip_prefix('=') {
                        let cmd = raw_value.trim();
                        let args = cached_shell_command(cmd, run_shell_for_flags, ctx)?;
                        match *var_name {
                            "EXTRA_COMPILE_SHELL" => flags.compile_args_after.extend(args),
                            "EXTRA_LINK_SHELL" => flags.link_args_after.extend(args),
                            _ => {}
                        }
                    }
        }
    }

    Ok(flags)
}

/// Match a directive with optional profile suffix.
/// Returns `Some((rest_of_line`, applies)) if the directive matches.
/// - `applies` is true if the directive applies to the current profile
///   (either no profile suffix, or profile suffix matches current profile)
/// - `rest_of_line` is everything after the directive name (and optional profile suffix)
///
/// Examples:
///   "EXTRA_COMPILE_FLAGS_BEFORE=-g" with profile "gcc" -> Some(("=-g", true))
///   "`EXTRA_COMPILE_FLAGS_BEFORE`[gcc]=-g" with profile "gcc" -> Some(("=-g", true))
///   "`EXTRA_COMPILE_FLAGS_BEFORE`[clang]=-g" with profile "gcc" -> Some(("=-g", false))
fn match_directive_with_profile<'a>(line: &'a str, directive: &str, profile_name: &str) -> Option<(&'a str, bool)> {
    if let Some(rest) = line.strip_prefix(directive) {
        // Check for profile suffix [profile_name]
        if let Some(rest_after_bracket) = rest.strip_prefix('[') {
            // Find closing bracket
            if let Some(bracket_end) = rest_after_bracket.find(']') {
                let specified_profile = &rest_after_bracket[..bracket_end];
                let rest_after_suffix = &rest_after_bracket[bracket_end + 1..];
                let applies = specified_profile == profile_name;
                return Some((rest_after_suffix, applies));
            }
            // Malformed (no closing bracket), don't match
            None
        } else {
            // No profile suffix - applies to all profiles
            Some((rest, true))
        }
    } else {
        None
    }
}

/// Run a command as a subprocess and return its stdout split into args.
/// The value is split on whitespace: first token is the program, rest are arguments.
///
/// These are user-specified commands (EXTRA_*_CMD directives), so we temporarily
/// suspend the declared tools check to allow arbitrary programs.
fn run_command_for_flags(ctx: &crate::build_context::BuildContext, cmd_line: &str) -> Result<Vec<String>> {
    let parts: Vec<&str> = cmd_line.split_whitespace().collect();
    if parts.is_empty() {
        return Ok(Vec::new());
    }
    let program = parts[0];
    let args = &parts[1..];

    let mut cmd = Command::new(program);
    cmd.args(args);
    let _guard = crate::processors::suspend_tool_check();
    let output = run_command_capture(ctx, &cmd)?;
    check_command_output(&output, cmd_line)?;

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    Ok(stdout.split_whitespace().map(String::from).collect())
}

/// Run a command via `sh -c` and return stdout split into flags.
///
/// These are user-specified commands (EXTRA_*_SHELL directives), so we temporarily
/// suspend the declared tools check to allow arbitrary programs.
fn run_shell_for_flags(ctx: &crate::build_context::BuildContext, cmd_line: &str) -> Result<Vec<String>> {
    if cmd_line.is_empty() {
        return Ok(Vec::new());
    }

    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(cmd_line);
    let _guard = crate::processors::suspend_tool_check();
    let output = run_command_capture(ctx, &cmd)?;
    check_command_output(&output, cmd_line)?;

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    Ok(stdout.split_whitespace().map(String::from).collect())
}

/// Run a backtick command and return its output as a single string.
///
/// These are user-specified commands (backtick expansion), so we temporarily
/// suspend the declared tools check to allow arbitrary programs.
fn run_backtick_command(ctx: &crate::build_context::BuildContext, cmd_str: &str) -> Result<Vec<String>> {
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(cmd_str);
    let _guard = crate::processors::suspend_tool_check();
    let output = run_command_capture(ctx, &cmd)?;
    check_command_output(&output, cmd_str)?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    // Return as single-element vec so caching works uniformly
    Ok(vec![stdout])
}

/// Expand backtick-wrapped portions in a string by running them as shell commands.
/// E.g. "`pkg-config --cflags gtk+-3.0`" → the stdout of that command.
/// Results are cached to avoid running the same command multiple times.
fn expand_backticks(ctx: &crate::build_context::BuildContext, value: &str) -> Result<String> {
    if !value.contains('`') {
        return Ok(value.to_string());
    }

    let mut result = String::new();
    let mut rest = value;

    while let Some(start) = rest.find('`') {
        result.push_str(&rest[..start]);
        let after_start = &rest[start + 1..];
        let end = after_start.find('`').ok_or_else(|| {
            anyhow::anyhow!("Unmatched backtick in value: {value}")
        })?;
        let cmd_str = &after_start[..end];
        // Use cache for backtick commands too
        let cached = cached_shell_command(cmd_str, run_backtick_command, ctx)?;
        let stdout = cached.first().map(std::string::String::as_str).unwrap_or("");
        result.push_str(stdout);
        rest = &after_start[end + 1..];
    }
    result.push_str(rest);

    Ok(result)
}


use std::path::PathBuf;

use crate::config::{CcSingleFileConfig, CompilerProfile, output_config_hash, resolve_extra_inputs};
use crate::file_index::FileIndex;
use crate::graph::{BuildGraph, Product};
use crate::processors::{Processor, format_command, run_command};


pub struct CcSingleFileProcessor {
    config: CcSingleFileConfig,
    profiles: Vec<CompilerProfile>,
    output_dir: PathBuf,
}

impl CcSingleFileProcessor {
    pub fn new(config: CcSingleFileConfig) -> Self {
        let profiles = config.get_compiler_profiles();
        let output_dir = PathBuf::from(&config.standard.output_dir);
        Self {
            config,
            profiles,
            output_dir,
        }
    }

    /// Get the first source directory from scan config (relative path)
    fn source_dir(&self) -> PathBuf {
        PathBuf::from(self.config.standard.src_dirs().first().map(std::string::String::as_str).unwrap_or(""))
    }

    /// Check if cc processing should be enabled
    fn should_process(&self) -> bool {
        let src = self.source_dir();
        src.as_os_str().is_empty() || src.exists()
    }

    /// Find all C/C++ source files. Returns (path, `is_cpp`) pairs.
    fn find_source_files(&self, file_index: &FileIndex) -> Vec<(PathBuf, bool)> {
        file_index.scan(&self.config.standard, true)
            .into_iter()
            .map(|p| {
                let is_cpp = p.extension().and_then(|s| s.to_str()) == Some("cc");
                (p, is_cpp)
            })
            .collect()
    }

    /// Get executable path for a source file with a specific compiler profile.
    /// Preserves the full source path relative to project root.
    /// E.g., src/a.cc -> `out/cc_single_file/src/a.elf`
    /// With profile: src/a.cc -> `out/cc_single_file`/<`profile_name>/src/a.elf`
    fn get_executable_path(&self, source: &Path, profile: &CompilerProfile) -> PathBuf {
        // Keep the full source path, just change the extension
        let stem = source.with_extension("");
        let name = format!("{}{}", stem.display(), profile.output_suffix);

        if profile.name.is_empty() {
            self.output_dir.join(name)
        } else {
            self.output_dir.join(&profile.name).join(name)
        }
    }

    /// Find a compiler profile by name
    fn find_profile(&self, name: &str) -> Option<&CompilerProfile> {
        self.profiles.iter().find(|p| p.name == name)
    }

    /// Add include paths and compile flags (before, base, after) to a command.
    fn add_compile_flags(&self, cmd: &mut Command, profile: &CompilerProfile, is_cpp: bool, source_flags: &SourceFlags) {
        let flags = if is_cpp { &profile.cxxflags } else { &profile.cflags };
        for inc in &self.config.include_paths {
            cmd.arg(format!("-I{inc}"));
        }
        for arg in &source_flags.compile_args_before {
            cmd.arg(arg);
        }
        for flag in flags {
            cmd.arg(flag);
        }
        for arg in &source_flags.compile_args_after {
            cmd.arg(arg);
        }
    }

    /// Compile a single source file directly to an executable using a specific profile.
    fn compile_source(&self, ctx: &crate::build_context::BuildContext, source: &Path, executable: &Path, profile: &CompilerProfile, is_cpp: bool) -> Result<()> {
        let compiler = if is_cpp { &profile.cxx } else { &profile.cc };
        let source_flags = parse_source_flags(ctx, source, &profile.name)?;

        // Ensure output directory exists
        crate::processors::ensure_output_dir(executable)?;

        let mut cmd = Command::new(compiler);
        self.add_compile_flags(&mut cmd, profile, is_cpp, &source_flags);
        cmd.arg("-o").arg(executable).arg(source);
        for arg in &source_flags.link_args_before {
            cmd.arg(arg);
        }
        for flag in &profile.ldflags {
            cmd.arg(flag);
        }
        for arg in &source_flags.link_args_after {
            cmd.arg(arg);
        }

        if crate::runtime_flags::show_child_processes() {
            let profile_tag = if profile.name.is_empty() { String::new() } else { format!(":{}", profile.name) };
            println!("[{}{}] {}", crate::processors::names::CC_SINGLE_FILE, profile_tag, format_command(&cmd));
        }

        let output = run_command(ctx, &cmd)?;
        check_command_output(&output, format_args!("Compilation of {}", source.display()))
    }

    /// Extract profile name from product metadata
    fn get_profile_from_product(&self, product: &Product) -> Result<&CompilerProfile> {
        // Profile name is stored in the output path structure
        // out/cc_single_file/<profile_name>/... or out/cc_single_file/... (legacy)
        if let Some(output) = product.outputs.first()
            && let Ok(relative) = output.strip_prefix(&self.output_dir) {
                // Check if first component is a profile name
                if let Some(first) = relative.components().next() {
                    let first_str = first.as_os_str().to_string_lossy();
                    if let Some(profile) = self.find_profile(&first_str) {
                        return Ok(profile);
                    }
                }
            }
        // Fall back to first profile (legacy mode)
        self.profiles.first()
            .ok_or_else(|| anyhow::anyhow!("no compiler profiles configured"))
    }

    /// Shared implementation for discover and `discover_for_clean`.
    /// When `for_clean` is true, skips config hash and extra inputs (only needs output mapping).
    fn discover_impl(&self, graph: &mut BuildGraph, file_index: &FileIndex, for_clean: bool, instance_name: &str) -> Result<()> {
        if !self.should_process() {
            return Ok(());
        }

        let source_files = self.find_source_files(file_index);
        if source_files.is_empty() {
            return Ok(());
        }

        let cfg_hash = if for_clean { None } else { Some(output_config_hash(&self.config, <crate::config::CcSingleFileConfig as crate::config::KnownFields>::checksum_fields())) };
        let extra = if for_clean { Vec::new() } else { resolve_extra_inputs(&self.config.standard.dep_inputs)? };

        for profile in &self.profiles {
            let variant = if profile.name.is_empty() { None } else { Some(profile.name.as_str()) };

            for (source, _is_cpp) in &source_files {
                if should_exclude_for_profile(source, &profile.name) {
                    continue;
                }

                let executable = self.get_executable_path(source, profile);

                let mut inputs = Vec::with_capacity(1 + extra.len());
                inputs.push(source.clone());
                inputs.extend_from_slice(&extra);

                graph.add_product_with_variant(
                    inputs,
                    vec![executable],
                    instance_name,
                    cfg_hash.clone(),
                    variant,
                )?;
            }
        }

        Ok(())
    }
}

impl Processor for CcSingleFileProcessor {
    fn scan_config(&self) -> &crate::config::StandardConfig {
        &self.config.standard
    }


    fn config_json(&self) -> Option<String> {
        crate::processors::ProcessorBase::config_json(&self.config)
    }

    fn clean(&self, product: &crate::graph::Product, verbose: bool) -> anyhow::Result<usize> {
        crate::processors::ProcessorBase::clean(product, &product.processor, verbose)
    }

    fn auto_detect(&self, file_index: &FileIndex) -> bool {
        self.should_process() && !self.find_source_files(file_index).is_empty()
    }

    fn required_tools(&self) -> Vec<String> {
        // Collect unique compilers from all profiles
        let mut tools: Vec<String> = self.profiles.iter()
            .flat_map(|p| vec![p.cc.clone(), p.cxx.clone()])
            .collect();
        tools.sort();
        tools.dedup();
        tools
    }

    fn discover(&self, graph: &mut BuildGraph, file_index: &FileIndex, instance_name: &str) -> Result<()> {
        self.discover_impl(graph, file_index, false, instance_name)
    }

    /// Fast discovery for clean: only find outputs, skip header scanning
    fn discover_for_clean(&self, graph: &mut BuildGraph, file_index: &FileIndex, instance_name: &str) -> Result<()> {
        self.discover_impl(graph, file_index, true, instance_name)
    }

    fn execute(&self, ctx: &crate::build_context::BuildContext, product: &Product) -> Result<()> {
        let source = product.primary_input();
        let executable = product.primary_output();
        let is_cpp = source.extension().and_then(|s| s.to_str()) == Some("cc");
        let profile = self.get_profile_from_product(product)?;
        self.compile_source(ctx, source, executable, profile, is_cpp)
    }

}

fn plugin_create(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(CcSingleFileProcessor::new(cfg)))
}
inventory::submit! {
    crate::registries::ProcessorPlugin {
        version: 1,
        name: "cc_single_file",
        processor_type: crate::processors::ProcessorType::Generator,
        create: plugin_create,
        defconfig_json: crate::registries::default_config_json::<crate::config::CcSingleFileConfig>,
        known_fields: crate::registries::typed_known_fields::<crate::config::CcSingleFileConfig>,
        checksum_fields: crate::registries::typed_checksum_fields::<crate::config::CcSingleFileConfig>,
        must_fields: crate::registries::typed_must_fields::<crate::config::CcSingleFileConfig>,
        field_descriptions: crate::registries::typed_field_descriptions::<crate::config::CcSingleFileConfig>,
        keywords: &["c", "cpp", "compiler", "gcc", "clang", "binary", "executable"],
        description: "Compile C/C++ source files into executables (single-file)",
        is_native: false,
        can_fix: false,
        supports_batch: false,
        max_jobs_cap: None,
    }
}
