use anyhow::{Context, Result};
use chrono::Datelike;
use mlua::LuaSerdeExt;
use mlua::prelude::*;
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tera::{Context as TeraContext, Function, Tera, Value as TeraValue, to_value};

use serde::{Deserialize, Serialize};

use crate::config::{StandardConfig, output_config_hash, resolve_extra_inputs};
use crate::file_index::FileIndex;
use crate::graph::{BuildGraph, Product};
use crate::processors::{Processor, run_command_capture};

use super::TemplateItem;

/// Tera template processor config. No custom fields.
/// Unused `StandardConfig` fields: command, formats, `output_dir`.
#[derive(Debug, Deserialize, Serialize, Clone)]
#[derive(Default)]
pub struct TeraConfig {
    #[serde(flatten)]
    pub standard: StandardConfig,
}

/// Wrapper around a `&BuildContext` reference that can be stored in Tera function structs.
/// Tera's `Function` trait requires `Send + Sync + 'static`, so we cannot use a borrow.
/// Safety: the pointer is only dereferenced during `render_template`, which holds the
/// original `&BuildContext` reference for the entire duration of the Tera render call.
#[derive(Clone, Copy)]
struct CtxPtr(*const crate::build_context::BuildContext);

// SAFETY: BuildContext is Sync + Send; the pointer is only live while render_template runs.
unsafe impl Send for CtxPtr {}
unsafe impl Sync for CtxPtr {}

impl CtxPtr {
    const fn get(&self) -> &crate::build_context::BuildContext {
        // SAFETY: caller guarantees the BuildContext outlives all uses of CtxPtr.
        unsafe { &*self.0 }
    }
}

/// Render a template item and write to output. `includable` is the set of
/// `.tera` files registered for `{% include %}` resolution — sourced from
/// the file index at discover time, so ignored trees (`node_modules`, vendored
/// code) never contribute templates.
fn render_template(ctx: &crate::build_context::BuildContext, item: &TemplateItem, includable: &[PathBuf]) -> Result<()> {
    // Ensure parent directory of output exists
    crate::processors::ensure_output_dir(&item.output_path)?;

    // Read template content
    let template_content = crate::errors::ctx(fs::read_to_string(&item.source_path), &format!("Failed to read template: {}", item.source_path.display()))?;

    // Create a new Tera instance for this template
    let mut tera = Tera::default();

    // Register template functions
    let ctx_ptr = CtxPtr(std::ptr::from_ref(ctx));
    tera.register_function("load_python", LoadPythonFunction { ctx: ctx_ptr });
    tera.register_function("load_lua", LoadLuaFunction);
    tera.register_function("version_str", VersionStrFunction { ctx: ctx_ptr });
    tera.register_function("copyright_years", CopyrightYearsFunction { ctx: ctx_ptr });
    tera.register_function("git_count_files", GitCountFilesFunction { ctx: ctx_ptr });
    tera.register_function("workflow_names", WorkflowNamesFunction);
    tera.register_function("shell_output", ShellOutputFunction { ctx: ctx_ptr });
    tera.register_function("glob", GlobFunction);
    tera.register_function("grep_count", GrepCountFunction);

    // Add the template
    tera.add_raw_template("template", &template_content)
        .context("Failed to parse template")?;

    // Register the project's .tera files so {% include %} can resolve them
    for path in includable {
        // Skip the main template we already registered
        if *path == item.source_path {
            continue;
        }
        let name = path.to_string_lossy().to_string();
        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read template: {}", path.display()))?;
        tera.add_raw_template(&name, &content)
            .with_context(|| format!("Failed to parse template: {}", path.display()))?;
    }

    // Configure strict mode (fail on undefined variables)
    tera.set_escape_fn(std::string::ToString::to_string); // No HTML escaping by default

    // Create an empty context (load_python will be called from within the template)
    let context = TeraContext::new();

    // Render the template
    let rendered = tera
        .render("template", &context)
        .with_context(|| format!("Failed to render template: {}", item.source_path.display()))?;

    // Write to output file
    crate::errors::ctx(fs::write(&item.output_path, rendered), &format!("Failed to write output: {}", item.output_path.display()))?;

    Ok(())
}

pub struct TeraProcessor {
    config: TeraConfig,
    /// All `.tera` files in the project, captured from the file index at
    /// discover time. `render_template` registers these so `{% include %}`
    /// resolves — reading them from the index (instead of re-globbing the
    /// filesystem per render, as this used to) honors
    /// `.gitignore`/`.rsconstructignore` and costs one walk, not one per
    /// product.
    includable_templates: std::sync::Mutex<Vec<PathBuf>>,
}

impl TeraProcessor {
    pub const fn new(config: TeraConfig) -> Self {
        Self {
            config,
            includable_templates: std::sync::Mutex::new(Vec::new()),
        }
    }
}

impl Processor for TeraProcessor {
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
        !super::find_templates(&self.config.standard, file_index).is_empty()
    }

    fn required_tools(&self) -> Vec<String> {
        vec!["python3".to_string(), "sh".to_string(), "git".to_string()]
    }

    fn discover(&self, graph: &mut BuildGraph, file_index: &FileIndex, instance_name: &str) -> Result<()> {
        let items = super::find_templates(&self.config.standard, file_index);
        let extra = resolve_extra_inputs(&self.config.standard.dep_inputs)?;

        // Capture the includable-template set for execute (see the field doc).
        *self.includable_templates.lock().unwrap() = file_index.files().iter()
            .filter(|p| p.to_string_lossy().ends_with(".tera"))
            .cloned()
            .collect();

        for item in items {
            let mut inputs = Vec::with_capacity(1 + extra.len());
            inputs.push(item.source_path.clone());
            inputs.extend_from_slice(&extra);
            graph.add_product(
                inputs,
                vec![item.output_path.clone()],
                instance_name,
                Some(output_config_hash(&self.config, &crate::config::checksum_fields_of(instance_name))),
            )?;
        }

        Ok(())
    }

    fn execute(&self, ctx: &crate::build_context::BuildContext, product: &Product) -> Result<()> {
        let item = TemplateItem::new(
            product.primary_input().to_path_buf(),
            product.primary_output().to_path_buf(),
        );
        let includable = self.includable_templates.lock().unwrap().clone();
        render_template(ctx, &item, &includable)
    }
}

/// Custom Tera function to load Python configuration files
struct LoadPythonFunction { ctx: CtxPtr }

impl Function for LoadPythonFunction {
    fn call(&self, args: &HashMap<String, TeraValue>) -> tera::Result<TeraValue> {
        // Get the path argument
        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| tera::Error::msg("load_python requires a 'path' argument"))?;

        // Execute Python and load the config
        let result = load_python_config(self.ctx.get(), Path::new(path))
            .map_err(|e| tera::Error::msg(format!("Failed to load Python config: {e}")))?;

        to_value(result).map_err(|e| tera::Error::msg(format!("Failed to convert Python config to template value: {e}")))
    }
}

/// Custom Tera function to load Lua configuration files
struct LoadLuaFunction;

impl Function for LoadLuaFunction {
    fn call(&self, args: &HashMap<String, TeraValue>) -> tera::Result<TeraValue> {
        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| tera::Error::msg("load_lua requires a 'path' argument"))?;

        let result = load_lua_config(Path::new(path))
            .map_err(|e| tera::Error::msg(format!("Failed to load Lua config: {e}")))?;

        to_value(result).map_err(|e| tera::Error::msg(format!("Failed to convert Lua config to template value: {e}")))
    }
}

/// Load a Python file containing a `tup` variable and return a dot-joined version string.
/// e.g. `tup = (0, 0, 1)` → `"0.0.1"`
struct VersionStrFunction { ctx: CtxPtr }

impl Function for VersionStrFunction {
    fn call(&self, args: &HashMap<String, TeraValue>) -> tera::Result<TeraValue> {
        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or("config/version.py");

        // Case-sensitive on purpose: `path` comes from the template author,
        // and every other extension check in this codebase treats the
        // lowercase form as canonical.
        #[allow(clippy::case_sensitive_file_extension_comparisons)]
        let config = if path.ends_with(".lua") {
            load_lua_config(Path::new(path))
        } else {
            load_python_config(self.ctx.get(), Path::new(path))
        }
        .map_err(|e| tera::Error::msg(format!("version_str: failed to load {path}: {e}")))?;

        let tup = config
            .get("tup")
            .and_then(|v| v.as_array())
            .ok_or_else(|| tera::Error::msg(format!("version_str: no 'tup' array in {path}")))?;

        let version: Vec<String> = tup
            .iter()
            .map(|v| {
                v.as_i64()
                    .map(|n| n.to_string())
                    .or_else(|| v.as_str().map(String::from))
                    .unwrap_or_default()
            })
            .collect();

        to_value(version.join("."))
            .map_err(|e| tera::Error::msg(format!("version_str: {e}")))
    }
}

/// Return a comma-separated range of years from the first git commit year to the current year.
/// e.g. `"2013, 2014, 2015, ..., 2026"`.
///
/// If the directory isn't a git repo or has no commits yet, falls back to just the
/// current year — a fresh project shouldn't fail to render its README on day zero.
struct CopyrightYearsFunction { ctx: CtxPtr }

impl Function for CopyrightYearsFunction {
    fn call(&self, _args: &HashMap<String, TeraValue>) -> tera::Result<TeraValue> {
        let current_year = chrono::Local::now().year();

        let mut cmd = Command::new("git");
        cmd.args(["log", "--reverse", "--format=%ad", "--date=format:%Y"]);
        // Falling back to the current year is only legitimate for a repo
        // with no history (fresh project, not a git repo). Every other
        // failure — git errored, unparseable date — must be surfaced: a
        // silent fallback renders "© 2026" instead of the full range, gets
        // cached, and differs between machines (e.g. shallow CI clones,
        // which this cannot detect at all).
        let first_year: i32 = match run_command_capture(self.ctx.get(), &cmd) {
            Ok(output) if output.status.success() => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                match stdout.lines().next() {
                    None => current_year, // repo with zero commits
                    Some(line) => line.trim().parse().map_err(|e| {
                        tera::Error::msg(format!("copyright_years: cannot parse git year {line:?}: {e}"))
                    })?,
                }
            }
            Ok(_) => {
                // Non-zero exit: not a repo / no HEAD yet. Legitimate
                // fallback, but say so — silence here masked real failures.
                crate::output::warn("copyright_years: git log failed (not a repository or no commits yet); using the current year");
                current_year
            }
            Err(e) => {
                return Err(tera::Error::msg(format!("copyright_years: failed to run git: {e}")));
            }
        };

        let first_year = first_year.min(current_year);
        let years: Vec<String> = (first_year..=current_year).map(|y| y.to_string()).collect();

        to_value(years.join(", "))
            .map_err(|e| tera::Error::msg(format!("copyright_years: {e}")))
    }
}

/// Run `git ls-files -- "{pattern}"` and return the count of matching files.
struct GitCountFilesFunction { ctx: CtxPtr }

impl Function for GitCountFilesFunction {
    fn call(&self, args: &HashMap<String, TeraValue>) -> tera::Result<TeraValue> {
        let pattern = args
            .get("pattern")
            .and_then(|v| v.as_str())
            .ok_or_else(|| tera::Error::msg("git_count_files requires a 'pattern' argument"))?;

        let mut cmd = Command::new("git");
        cmd.args(["ls-files", "--", pattern]);
        let output = run_command_capture(self.ctx.get(), &cmd)
            .map_err(|e| tera::Error::msg(format!("git_count_files: {e}")))?;

        if !output.status.success() {
            return Err(tera::Error::msg("git_count_files: git ls-files failed"));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let count = stdout.lines().filter(|l| !l.is_empty()).count();

        to_value(count)
            .map_err(|e| tera::Error::msg(format!("git_count_files: {e}")))
    }
}

/// Glob `.github/workflows/*.yml`, parse each YAML file's `name` field,
/// return array of objects `[{file: "build.yml", name: "build"}, ...]`.
struct WorkflowNamesFunction;

impl Function for WorkflowNamesFunction {
    fn call(&self, _args: &HashMap<String, TeraValue>) -> tera::Result<TeraValue> {
        let pattern = ".github/workflows/*.yml";
        let mut results = Vec::new();

        for entry in glob::glob(pattern)
            .map_err(|e| tera::Error::msg(format!("workflow_names: invalid glob: {e}")))?
        {
            let path = entry
                .map_err(|e| tera::Error::msg(format!("workflow_names: glob error: {e}")))?;

            let content = fs::read_to_string(&path)
                .map_err(|e| tera::Error::msg(format!("workflow_names: read {}: {e}", path.display())))?;

            let yaml: serde_yml::Value = serde_yml::from_str(&content)
                .map_err(|e| tera::Error::msg(format!("workflow_names: parse {}: {e}", path.display())))?;

            let name = yaml
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let file = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();

            let mut entry_map = Map::new();
            entry_map.insert("file".to_string(), Value::String(file));
            entry_map.insert("name".to_string(), Value::String(name));
            results.push(Value::Object(entry_map));
        }

        to_value(results)
            .map_err(|e| tera::Error::msg(format!("workflow_names: {e}")))
    }
}

/// Run a shell command and return its trimmed stdout. Requires `depends_on=[...]`
/// — a list of glob patterns whose union of resolved files determines the
/// build-graph dependency. Pass `depends_on=[]` to assert that the command
/// has no file-level dependencies rsconstruct can track.
///
/// The dependency tracking happens in the Tera analyzer
/// (`src/analyzers/tera.rs`); this function is responsible only for running
/// the command and returning the result. The arg validation here is a safety
/// net: rendering must not silently succeed when the user forgot `depends_on`,
/// even if (somehow) the analyzer didn't run for this template.
struct ShellOutputFunction { ctx: CtxPtr }

impl Function for ShellOutputFunction {
    fn call(&self, args: &HashMap<String, TeraValue>) -> tera::Result<TeraValue> {
        let command = args
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| tera::Error::msg("shell_output requires a 'command' argument"))?;

        // depends_on must be present (analyzer enforces this too, but we
        // double-check at render time so a missed analyzer pass doesn't lead
        // to silent stale output).
        let depends_on = args.get("depends_on").ok_or_else(|| {
            tera::Error::msg(format!(
                "shell_output(command=\"{command}\") requires depends_on=[...] — \
                 rsconstruct cannot otherwise tell when its output should be invalidated. \
                 Pass depends_on=[\"glob/pattern/*.ext\", ...] or depends_on=[] to \
                 acknowledge that no file dependencies exist.",
            ))
        })?;
        if !depends_on.is_array() {
            return Err(tera::Error::msg(format!(
                "shell_output(command=\"{command}\"): depends_on must be a list of strings, got {depends_on:?}",
            )));
        }

        let mut cmd = Command::new("sh");
        cmd.args(["-c", command]);
        let output = run_command_capture(self.ctx.get(), &cmd)
            .map_err(|e| tera::Error::msg(format!("shell_output: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(tera::Error::msg(format!("shell_output: command failed: {stderr}")));
        }

        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        to_value(stdout)
            .map_err(|e| tera::Error::msg(format!("shell_output: {e}")))
    }
}

/// Expand a glob pattern at template render time and return the sorted list of
/// matched file paths (relative to the project root, as strings). The same
/// pattern is independently captured by the Tera analyzer at graph-construction
/// time, which mixes the path set into the cache key (path-only, not content).
/// So a template that calls `glob()` is correctly invalidated when files
/// matching the pattern are added, removed, or renamed — but NOT when an
/// existing matching file's content changes.
struct GlobFunction;

impl Function for GlobFunction {
    fn call(&self, args: &HashMap<String, TeraValue>) -> tera::Result<TeraValue> {
        let pattern = args
            .get("pattern")
            .and_then(|v| v.as_str())
            .ok_or_else(|| tera::Error::msg("glob requires a 'pattern' argument"))?;

        let mut paths: Vec<String> = Vec::new();
        for entry in glob::glob(pattern)
            .map_err(|e| tera::Error::msg(format!("glob: invalid pattern '{pattern}': {e}")))?
        {
            let path = entry
                .map_err(|e| tera::Error::msg(format!("glob: iteration error for '{pattern}': {e}")))?;
            if path.is_file() {
                paths.push(path.to_string_lossy().into_owned());
            }
        }
        paths.sort();
        paths.dedup();

        to_value(paths)
            .map_err(|e| tera::Error::msg(format!("glob: {e}")))
    }
}

/// Count lines matching a regex across all files matching a glob pattern.
/// This is an in-process replacement for `shell_output(command="grep -r ... | wc -l")`
/// — same result, no shell or external grep involved. The analyzer captures
/// both the literal regex and the resolved file set into the cache hash, AND
/// adds the matched files as inputs so editing any of their contents
/// correctly invalidates the product.
struct GrepCountFunction;

impl Function for GrepCountFunction {
    fn call(&self, args: &HashMap<String, TeraValue>) -> tera::Result<TeraValue> {
        let pattern = args
            .get("pattern")
            .and_then(|v| v.as_str())
            .ok_or_else(|| tera::Error::msg("grep_count requires a 'pattern' argument (regex)"))?;
        let glob_pattern = args
            .get("glob")
            .and_then(|v| v.as_str())
            .ok_or_else(|| tera::Error::msg("grep_count requires a 'glob' argument (file glob)"))?;

        let re = regex::Regex::new(pattern)
            .map_err(|e| tera::Error::msg(format!("grep_count: invalid regex '{pattern}': {e}")))?;

        let mut count: usize = 0;
        for entry in glob::glob(glob_pattern)
            .map_err(|e| tera::Error::msg(format!("grep_count: invalid glob '{glob_pattern}': {e}")))?
        {
            let path = entry
                .map_err(|e| tera::Error::msg(format!("grep_count: glob error for '{glob_pattern}': {e}")))?;
            if !path.is_file() {
                continue;
            }
            let content = fs::read_to_string(&path)
                .map_err(|e| tera::Error::msg(format!("grep_count: read {}: {e}", path.display())))?;
            for line in content.lines() {
                if re.is_match(line) {
                    count += 1;
                }
            }
        }

        to_value(count)
            .map_err(|e| tera::Error::msg(format!("grep_count: {e}")))
    }
}

/// Load configuration from a Python file
fn load_python_config(ctx: &crate::build_context::BuildContext, python_file: &Path) -> Result<Map<String, Value>> {
    // Resolve the path relative to current working directory
    let absolute_path = if python_file.is_absolute() {
        python_file.to_path_buf()
    } else {
        std::env::current_dir()
            .context("Failed to get current directory to resolve Python config path")?
            .join(python_file)
    };

    if !absolute_path.exists() {
        anyhow::bail!("Python config file not found: {}", absolute_path.display());
    }

    // Create a Python script that will execute the config file and output variables as JSON.
    // Escape backslashes and single quotes for safe embedding in Python string literals.
    let config_dir = crate::processors::parent_dir(&absolute_path).display().to_string()
        .replace('\\', "\\\\").replace('\'', "\\'");
    let config_path = absolute_path.display().to_string()
        .replace('\\', "\\\\").replace('\'', "\\'");
    let python_script = format!(
        r"
import sys
import json
import os

# Set the working directory to the config file's directory
config_dir = '{config_dir}'
if config_dir:
    sys.path.insert(0, config_dir)

# Create a namespace for execution
namespace = {{}}

# Execute the config file
with open('{config_path}', 'r') as f:
    exec(f.read(), namespace)

# Filter out built-in variables and convert to JSON-serializable format
result = {{}}
for key, value in namespace.items():
    if not key.startswith('__'):
        try:
            # Try to serialize the value
            json.dumps(value)
            result[key] = value
        except:
            # If not serializable, convert to string
            result[key] = str(value)

print(json.dumps(result))
"
    );

    // Execute Python and capture output
    let mut cmd = Command::new("python3");
    cmd.arg("-c").arg(&python_script);
    let output = run_command_capture(ctx, &cmd)?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Python config execution failed: {stderr}");
    }

    // Parse the JSON output
    let stdout = String::from_utf8_lossy(&output.stdout);
    let variables: Map<String, Value> =
        crate::errors::ctx(serde_json::from_str(&stdout), "Failed to parse Python config output")?;

    Ok(variables)
}

/// Names of Lua built-in globals to skip when extracting user-defined variables.
const LUA_BUILTIN_GLOBALS: &[&str] = &[
    "string", "table", "math", "io", "os", "debug", "coroutine", "utf8", "package",
    "assert", "collectgarbage", "dofile", "error", "getmetatable", "ipairs", "load",
    "loadfile", "next", "pairs", "pcall", "print", "rawequal", "rawget", "rawlen",
    "rawset", "require", "select", "setmetatable", "tonumber", "tostring", "type",
    "warn", "xpcall",
];

/// Load configuration from a Lua file
fn load_lua_config(lua_file: &Path) -> Result<Map<String, Value>> {
    let absolute_path = if lua_file.is_absolute() {
        lua_file.to_path_buf()
    } else {
        std::env::current_dir()
            .context("Failed to get current directory to resolve Lua config path")?
            .join(lua_file)
    };

    if !absolute_path.exists() {
        anyhow::bail!("Lua config file not found: {}", absolute_path.display());
    }

    let lua = Lua::new();

    // Set up package.path so require() works relative to the file's directory
    if let Some(dir) = absolute_path.parent() {
        let package: LuaTable = lua.globals().get("package")
            .map_err(|e| anyhow::anyhow!("Failed to get Lua package table: {e}"))?;
        let new_path = format!("{}/?.lua;{}/?.lua", dir.display(), dir.display());
        package.set("path", new_path)
            .map_err(|e| anyhow::anyhow!("Failed to set Lua package.path: {e}"))?;
    }

    // Load and execute the Lua file
    let script = fs::read_to_string(&absolute_path)
        .with_context(|| format!("Failed to read Lua config: {}", absolute_path.display()))?;
    lua.load(&script)
        .set_name(absolute_path.to_string_lossy())
        .exec()
        .map_err(|e| anyhow::anyhow!("Failed to execute Lua config '{}': {e}", absolute_path.display()))?;

    // Extract user-defined globals
    let globals = lua.globals();
    let mut result = Map::new();

    for pair in globals.pairs::<String, LuaValue>() {
        let (key, value) = pair
            .map_err(|e| anyhow::anyhow!("Failed to iterate Lua globals: {e}"))?;

        // Skip built-in globals and names starting with _
        if key.starts_with('_') || LUA_BUILTIN_GLOBALS.contains(&key.as_str()) {
            continue;
        }

        // Skip functions and non-serializable types
        match &value {
            LuaValue::Function(_) | LuaValue::Thread(_) | LuaValue::UserData(_)
            | LuaValue::LightUserData(_) => continue,
            _ => {}
        }

        // Convert to serde_json::Value using mlua's serde support
        let json_value: Value = lua.from_value(value)
            .map_err(|e| anyhow::anyhow!("Failed to convert Lua global '{key}' to JSON: {e}"))?;
        result.insert(key, json_value);
    }

    Ok(result)
}

/// Documentation for one built-in Tera function. Shared source of truth used
/// by both the `register_function` calls in `render_template` (indirectly,
/// for the names) and the `rsconstruct functions list` CLI command.
pub struct TeraFunctionDoc {
    /// Function name as called from a template, e.g. `glob`.
    pub name: &'static str,
    /// One-line summary (shown in the list view).
    pub summary: &'static str,
    /// Argument signature, e.g. `pattern: string`.
    pub args: &'static str,
    /// Return type description, e.g. `array<string>` or `int`.
    pub returns: &'static str,
    /// How the analyzer tracks dependencies: which inputs and what enters
    /// the cache hash. This is the part users get wrong most often.
    pub dep_tracking: &'static str,
    /// Short usage example.
    pub example: &'static str,
}

pub static TERA_FUNCTIONS: &[TeraFunctionDoc] = &[
    TeraFunctionDoc {
        name: "load_python",
        summary: "Execute a Python file and expose its top-level variables to the template.",
        args: "path: string",
        returns: "object (variable name → JSON-serializable value)",
        dep_tracking: "The file at `path` is added as an input (content-tracked).",
        example: r#"{% set cfg = load_python(path="config/version.py") %}{{ cfg.version }}"#,
    },
    TeraFunctionDoc {
        name: "load_lua",
        summary: "Execute a Lua file and expose its globals to the template.",
        args: "path: string",
        returns: "object (global name → JSON-serializable value)",
        dep_tracking: "The file at `path` is added as an input (content-tracked).",
        example: r#"{% set cfg = load_lua(path="config/project.lua") %}{{ cfg.NAME }}"#,
    },
    TeraFunctionDoc {
        name: "version_str",
        summary: "Read a `tup` tuple from a Python or Lua file and return a dot-joined version string.",
        args: r#"path: string (defaults to "config/version.py")"#,
        returns: "string",
        dep_tracking: "The file at `path` is added as an input (content-tracked).",
        example: r#"{{ version_str(path="config/version.py") }}  {# e.g. "0.9.10" #}"#,
    },
    TeraFunctionDoc {
        name: "copyright_years",
        summary: "Return a comma-separated list of years from the first git commit year to the current year.",
        args: "(none)",
        returns: "string",
        dep_tracking: "Not tracked. Result depends only on git history and the current year; \
                       a forced rebuild is needed when crossing a New Year if no other input changed.",
        example: r#"© {{ copyright_years() }}  {# e.g. "2013, 2014, ..., 2026" #}"#,
    },
    TeraFunctionDoc {
        name: "git_count_files",
        summary: "Count git-tracked files matching a pathspec (excludes .gitignore'd / untracked).",
        args: "pattern: string (git pathspec)",
        returns: "int",
        dep_tracking: "Path-set only. The resolved tracked-file list goes into the cache hash; \
                       individual file content does NOT. Invalidates on commit/uncommit of matching files.",
        example: r#"{{ git_count_files(pattern="syllabi/*.md") }} syllabi"#,
    },
    TeraFunctionDoc {
        name: "workflow_names",
        summary: "List GitHub Actions workflow files under `.github/workflows/*.yml` with their `name` fields.",
        args: "(none)",
        returns: r"array<{file: string, name: string}>",
        dep_tracking: "The workflow files are content-tracked inputs and the resolved path set enters the cache hash — renaming a workflow's name: or adding/removing a file invalidates the product.",
        example: r"{% for wf in workflow_names() %}![{{ wf.name }}](.../{{ wf.file }}){% endfor %}",
    },
    TeraFunctionDoc {
        name: "shell_output",
        summary: "Run a shell command and return its trimmed stdout.",
        args: r"command: string, depends_on: array<string> (glob patterns; pass [] if none)",
        returns: "string",
        dep_tracking: "Content-tracked. Files matched by any pattern in `depends_on` are added \
                       as inputs. The literal command string is mixed into the cache hash so \
                       editing the command also invalidates. `depends_on` is REQUIRED.",
        example: r#"{{ shell_output(command="date -u +%Y-%m-%d", depends_on=[]) }}"#,
    },
    TeraFunctionDoc {
        name: "glob",
        summary: "Return the sorted list of file paths matching a glob pattern.",
        args: "pattern: string (glob, e.g. `data/**/*.md`)",
        returns: "array<string>",
        dep_tracking: "Path-set only. The resolved file list goes into the cache hash; \
                       individual file content does NOT. Invalidates on add/remove/rename.",
        example: r#"{{ glob(pattern="data/**/*.md") | length }} data files"#,
    },
    TeraFunctionDoc {
        name: "grep_count",
        summary: "Count lines matching a regex across all files matching a glob (in-process; no shell).",
        args: r"pattern: string (regex), glob: string (file glob)",
        returns: "int",
        dep_tracking: "Content-tracked. Files matched by `glob` are added as inputs. The regex \
                       literal and resolved file set are mixed into the cache hash.",
        example: r#"{{ grep_count(pattern="^TODO", glob="src/**/*.rs") }} TODOs"#,
    },
];

use crate::registries as registry;

fn plugin_create(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    registry::deserialize_and_create(toml, |cfg| Box::new(TeraProcessor::new(cfg)))
}

inventory::submit! {
    registry::ProcessorPlugin {
        version: 1,
        name: "tera",
        processor_type: crate::processors::ProcessorType::Generator,
        create: plugin_create,
        fields: &[],
        omit_standard_fields: &[],
        scan_defaults: Some(crate::config::ScanDefaultsData { src_dirs: &[], src_extensions: &[".tera"], src_exclude_dirs: &[] }),
        defaults: None,
        defconfig_json: registry::default_config_json::<TeraConfig>,
        keywords: &["template", "generator", "jinja", "html", "rust"],
        description: "Render Tera templates into output files",
        is_native: true,
        can_fix: false,
        supports_batch: false,
        max_jobs_cap: None,
    }
}
