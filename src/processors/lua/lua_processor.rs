use anyhow::Result;
use mlua::prelude::*;
use parking_lot::Mutex;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::{output_config_hash, standard_config_from_toml, StandardConfig};
use crate::file_index::FileIndex;
use crate::graph::{BuildGraph, Product};
use crate::processors::{clean_outputs, ensure_stub_dir, run_command, Processor};

/// Convert a `LuaResult` to an `anyhow::Result` with a contextual message.
fn lua_context<T>(result: LuaResult<T>, msg: impl std::fmt::Display) -> Result<T> {
    result.map_err(|e| anyhow::anyhow!("{msg}: {e}"))
}

/// Wrapper around a `&BuildContext` pointer stored in Lua app data.
/// Safety: the pointer is only dereferenced during `execute()`, which holds the
/// original `&BuildContext` reference for the entire duration of the Lua call.
#[derive(Clone, Copy)]
struct CtxPtr(*const crate::build_context::BuildContext);

unsafe impl Send for CtxPtr {}
unsafe impl Sync for CtxPtr {}

impl CtxPtr {
    #[allow(dead_code)]
    const fn get(&self) -> &crate::build_context::BuildContext {
        // SAFETY: caller guarantees the BuildContext outlives all uses.
        unsafe { &*self.0 }
    }
}

/// Retrieve the `BuildContext` from Lua app data. Returns a Lua error when
/// called outside `execute()` — e.g. from `clean()` or `auto_detect()` — instead of
/// panicking or dereferencing a stale pointer.
///
/// SAFETY: The raw pointer stored in `CtxPtr` is valid because it is set at the
/// start of `execute()`, which holds a &`BuildContext` for the entire Lua call,
/// and removed again before `execute()` returns.
fn get_ctx_from_lua(lua: &Lua) -> Result<&crate::build_context::BuildContext, LuaError> {
    let guard = lua.app_data_ref::<CtxPtr>().ok_or_else(|| LuaError::external(
        "rsconstruct.run_command is only available during execute()"
    ))?;
    Ok(unsafe { &*guard.0 })
}

pub struct LuaProcessor {
    name: String,
    lua: Mutex<Lua>,
    stub_dir: PathBuf,
    config_value: toml::Value,
    scan_config: StandardConfig,
}

impl LuaProcessor {
    /// Create a new `LuaProcessor` from a plugin script file.
    pub fn new(
        name: String,
        script_path: &Path,
        config_value: toml::Value,
    ) -> Result<Self> {
        let lua = Lua::new();

        // Register the rsconstruct API before loading the script
        Self::register_api(&lua, &name)?;

        // Load and execute the Lua script
        let script = fs::read_to_string(script_path)
            .map_err(|e| anyhow::anyhow!("Failed to read Lua plugin '{}': {}", script_path.display(), e))?;
        lua_context(
            lua.load(&script).set_name(script_path.to_string_lossy()).exec(),
            format!("Failed to load Lua plugin '{name}'"),
        )?;

        // Extract scan config from the TOML config value
        let scan_config = standard_config_from_toml(&config_value, &[], &[], &[]);

        let stub_dir = PathBuf::from("out").join(&name);

        Ok(Self {
            name,
            lua: Mutex::new(lua),
            stub_dir,
            config_value,
            scan_config,
        })
    }

    /// Discover all Lua plugins in the plugins directory.
    /// Returns a Vec of (name, `LuaProcessor`) pairs.
    pub fn discover_plugins(
        plugins_dir: &str,
        extra_configs: &std::collections::HashMap<String, toml::Value>,
    ) -> Result<Vec<(String, Self)>> {
        let dir = Path::new(plugins_dir);
        if !dir.is_dir() {
            return Ok(Vec::new());
        }

        let mut plugins = Vec::new();
        let mut entries: Vec<_> = fs::read_dir(dir)
            .map_err(|e| anyhow::anyhow!("Failed to read plugins directory '{}': {}", dir.display(), e))?
            .filter_map(std::result::Result::ok)
            .collect();
        entries.sort_by_key(std::fs::DirEntry::file_name);

        for entry in entries {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("lua") {
                let name = path.file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_string();
                if name.is_empty() {
                    continue;
                }

                let config_value = extra_configs
                    .get(&name)
                    .cloned()
                    .unwrap_or_else(|| toml::Value::Table(toml::map::Map::new()));

                let proc = Self::new(
                    name.clone(),
                    &path,
                    config_value,
                )?;
                plugins.push((name, proc));
            }
        }

        Ok(plugins)
    }

    /// Register the `rsconstruct` global table with helper functions.
    fn register_api(lua: &Lua, proc_name: &str) -> Result<()> {
        let rsconstruct = lua_context(lua.create_table(), "Failed to create rsconstruct table")?;

        // rsconstruct.stub_path(source, suffix) - paths are relative to project root
        let stub_path_fn = lua_context(
            lua.create_function(|_, (source, suffix): (String, String)| {
                let src = PathBuf::from(&source);
                let stub_dir = PathBuf::from("out").join(&suffix);
                let stub = crate::processors::stub_path(&stub_dir, &src, &suffix);
                Ok(stub.to_string_lossy().to_string())
            }),
            "Failed to create stub_path function",
        )?;
        lua_context(rsconstruct.set("stub_path", stub_path_fn), "Failed to set stub_path")?;

        // rsconstruct.run_command(program, args)
        let run_cmd_fn = lua_context(
            lua.create_function(|lua, (program, args): (String, LuaTable)| {
                let ctx = get_ctx_from_lua(lua)?;
                let mut cmd = Command::new(&program);
                for i in 1..=args.len()? {
                    let arg: String = args.get(i)?;
                    cmd.arg(&arg);
                }
                let output = run_command(ctx, &cmd).map_err(|e| {
                    LuaError::external(format!("Failed to run '{program}': {e}"))
                })?;
                if !output.status.success() {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return Err(LuaError::external(format!(
                        "'{}' failed (exit {}):\n{}{}",
                        program,
                        output.status.code().unwrap_or(-1),
                        stdout,
                        stderr,
                    )));
                }
                Ok(())
            }),
            "Failed to create run_command function",
        )?;
        lua_context(rsconstruct.set("run_command", run_cmd_fn), "Failed to set run_command")?;

        // rsconstruct.run_command_cwd(program, args, cwd)
        let run_cmd_cwd_fn = lua_context(
            lua.create_function(|lua, (program, args, cwd): (String, LuaTable, String)| {
                let ctx = get_ctx_from_lua(lua)?;
                let mut cmd = Command::new(&program);
                for i in 1..=args.len()? {
                    let arg: String = args.get(i)?;
                    cmd.arg(&arg);
                }
                cmd.current_dir(&cwd);
                let output = run_command(ctx, &cmd).map_err(|e| {
                    LuaError::external(format!("Failed to run '{program}': {e}"))
                })?;
                if !output.status.success() {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return Err(LuaError::external(format!(
                        "'{}' failed (exit {}):\n{}{}",
                        program,
                        output.status.code().unwrap_or(-1),
                        stdout,
                        stderr,
                    )));
                }
                Ok(())
            }),
            "Failed to create run_command_cwd function",
        )?;
        lua_context(rsconstruct.set("run_command_cwd", run_cmd_cwd_fn), "Failed to set run_command_cwd")?;

        // rsconstruct.write_stub(path, content)
        let write_stub_fn = lua_context(
            lua.create_function(|_, (path, content): (String, String)| {
                let p = PathBuf::from(&path);
                if let Some(parent) = p.parent() {
                    fs::create_dir_all(parent).map_err(|e| {
                        LuaError::external(format!("Failed to create directory for stub: {e}"))
                    })?;
                }
                fs::write(&p, &content).map_err(|e| {
                    LuaError::external(format!("Failed to write stub '{path}': {e}"))
                })?;
                Ok(())
            }),
            "Failed to create write_stub function",
        )?;
        lua_context(rsconstruct.set("write_stub", write_stub_fn), "Failed to set write_stub")?;

        // rsconstruct.remove_file(path)
        let remove_file_fn = lua_context(
            lua.create_function(|_, path: String| {
                let p = PathBuf::from(&path);
                if p.exists() {
                    fs::remove_file(&p).map_err(|e| {
                        LuaError::external(format!("Failed to remove '{path}': {e}"))
                    })?;
                }
                Ok(())
            }),
            "Failed to create remove_file function",
        )?;
        lua_context(rsconstruct.set("remove_file", remove_file_fn), "Failed to set remove_file")?;

        // rsconstruct.file_exists(path)
        let file_exists_fn = lua_context(
            lua.create_function(|_, path: String| {
                Ok(PathBuf::from(&path).exists())
            }),
            "Failed to create file_exists function",
        )?;
        lua_context(rsconstruct.set("file_exists", file_exists_fn), "Failed to set file_exists")?;

        // rsconstruct.read_file(path)
        let read_file_fn = lua_context(
            lua.create_function(|_, path: String| {
                let content = fs::read_to_string(&path).map_err(|e| {
                    LuaError::external(format!("Failed to read '{path}': {e}"))
                })?;
                Ok(content)
            }),
            "Failed to create read_file function",
        )?;
        lua_context(rsconstruct.set("read_file", read_file_fn), "Failed to set read_file")?;

        // rsconstruct.path_join(parts...) - takes a table of path components
        let path_join_fn = lua_context(
            lua.create_function(|_, parts: LuaTable| {
                let mut path = PathBuf::new();
                for i in 1..=parts.len()? {
                    let part: String = parts.get(i)?;
                    path.push(&part);
                }
                Ok(path.to_string_lossy().to_string())
            }),
            "Failed to create path_join function",
        )?;
        lua_context(rsconstruct.set("path_join", path_join_fn), "Failed to set path_join")?;

        // rsconstruct.log(message)
        let proc_name_owned = proc_name.to_string();
        let log_fn = lua_context(
            lua.create_function(move |_, message: String| {
                println!("[{proc_name_owned}] {message}");
                Ok(())
            }),
            "Failed to create log function",
        )?;
        lua_context(rsconstruct.set("log", log_fn), "Failed to set log")?;

        lua_context(lua.globals().set("rsconstruct", rsconstruct), "Failed to set rsconstruct global")?;

        Ok(())
    }

    /// Convert a `toml::Value` to a Lua value for passing config to Lua functions.
    fn toml_to_lua(lua: &Lua, value: &toml::Value) -> LuaResult<LuaValue> {
        match value {
            toml::Value::String(s) => Ok(LuaValue::String(lua.create_string(s)?)),
            toml::Value::Integer(i) => Ok(LuaValue::Integer(*i)),
            toml::Value::Float(f) => Ok(LuaValue::Number(*f)),
            toml::Value::Boolean(b) => Ok(LuaValue::Boolean(*b)),
            toml::Value::Array(arr) => {
                let table = lua.create_table()?;
                for (i, v) in arr.iter().enumerate() {
                    table.set(i + 1, Self::toml_to_lua(lua, v)?)?;
                }
                Ok(LuaValue::Table(table))
            }
            toml::Value::Table(map) => {
                let table = lua.create_table()?;
                for (k, v) in map {
                    table.set(k.as_str(), Self::toml_to_lua(lua, v)?)?;
                }
                Ok(LuaValue::Table(table))
            }
            toml::Value::Datetime(dt) => Ok(LuaValue::String(lua.create_string(dt.to_string())?)),
        }
    }

    /// Check if a Lua global function exists.
    fn has_function(&self, name: &str) -> bool {
        self.lua.lock().globals()
            .get::<LuaFunction>(name)
            .is_ok()
    }

    /// Build a Lua table representing a product (inputs + outputs as string arrays).
    fn product_to_lua(lua: &Lua, product: &Product) -> Result<LuaTable> {
        let product_table = lua_context(lua.create_table(), "Failed to create product table")?;

        let inputs_table = lua_context(lua.create_table(), "Failed to create inputs table")?;
        for (i, input) in product.inputs.iter().enumerate() {
            lua_context(
                inputs_table.set(i + 1, input.to_string_lossy().to_string()),
                "Failed to set input",
            )?;
        }
        lua_context(product_table.set("inputs", inputs_table), "Failed to set inputs")?;

        let outputs_table = lua_context(lua.create_table(), "Failed to create outputs table")?;
        for (i, output) in product.outputs.iter().enumerate() {
            lua_context(
                outputs_table.set(i + 1, output.to_string_lossy().to_string()),
                "Failed to set output",
            )?;
        }
        lua_context(product_table.set("outputs", outputs_table), "Failed to set outputs")?;

        Ok(product_table)
    }
}

impl Processor for LuaProcessor {
    fn scan_config(&self) -> &crate::config::StandardConfig {
        &self.scan_config
    }


    fn discover(&self, graph: &mut BuildGraph, file_index: &FileIndex, instance_name: &str) -> Result<()> {
        let files = file_index.scan(&self.scan_config, true);
        if files.is_empty() {
            return Ok(());
        }

        let lua = self.lua.lock();

        // Build the files list as Lua strings
        let files_table = lua_context(lua.create_table(), "Failed to create files table")?;
        for (i, file) in files.iter().enumerate() {
            lua_context(
                files_table.set(i + 1, file.to_string_lossy().to_string()),
                "Failed to set file in table",
            )?;
        }

        // Convert config to Lua
        let config_lua = lua_context(
            Self::toml_to_lua(&lua, &self.config_value),
            format!("Failed to convert config for plugin '{}'", self.name),
        )?;

        // Call Lua discover(project_root, config, files)
        // project_root is always "." since RSConstruct runs from the project root
        let discover_fn: LuaFunction = lua_context(
            lua.globals().get("discover"),
            format!("Lua plugin '{}' must define a discover() function", self.name),
        )?;

        let products_table: LuaTable = lua_context(
            discover_fn.call((".".to_string(), config_lua, files_table)),
            format!("Lua plugin '{}': discover() failed", self.name),
        )?;

        // Lua plugins don't have a typed `KnownFields` impl. With the allowlist
        // hashing API, we synthesize an allowlist at runtime from every top-level
        // key present in the TOML value — so the hash behavior matches "hash the
        // whole config," which is the safest default for arbitrary user plugins.
        let keys: Vec<String> = self.config_value.as_table()
            .map(|t| t.keys().cloned().collect())
            .unwrap_or_default();
        let key_refs: Vec<&str> = keys.iter().map(std::string::String::as_str).collect();
        let hash = Some(output_config_hash(&self.config_value, &key_refs));

        // Parse each product from the returned table
        let len = lua_context(products_table.len(), "Failed to get products length")?;
        for i in 1..=len {
            let product: LuaTable = lua_context(products_table.get(i), "Failed to get product")?;

            let inputs_table: LuaTable = lua_context(product.get("inputs"), "Failed to get inputs")?;
            let outputs_table: LuaTable = lua_context(product.get("outputs"), "Failed to get outputs")?;

            let mut inputs = Vec::new();
            let inputs_len = lua_context(inputs_table.len(), "Failed to get inputs length")?;
            for j in 1..=inputs_len {
                let path: String = lua_context(inputs_table.get(j), "Failed to get input path")?;
                inputs.push(PathBuf::from(path));
            }

            let mut outputs = Vec::new();
            let outputs_len = lua_context(outputs_table.len(), "Failed to get outputs length")?;
            for j in 1..=outputs_len {
                let path: String = lua_context(outputs_table.get(j), "Failed to get output path")?;
                outputs.push(PathBuf::from(path));
            }

            graph.add_product(inputs, outputs, instance_name, hash.clone())?;
        }

        drop(lua);
        Ok(())
    }

    fn execute(&self, ctx: &crate::build_context::BuildContext, product: &Product) -> Result<()> {
        ensure_stub_dir(&self.stub_dir, &self.name)?;

        let lua = self.lua.lock();
        lua.set_app_data(CtxPtr(ctx as *const _));

        // The pointer must not outlive this call: clear it on every exit path
        // so later callbacks (clean, auto_detect) can't dereference a stale
        // BuildContext.
        let result = Self::product_to_lua(&lua, product).and_then(|product_table| {
            let execute_fn: LuaFunction = lua_context(
                lua.globals().get("execute"),
                format!("Lua plugin '{}' must define an execute() function", self.name),
            )?;
            lua_context(
                execute_fn.call::<()>(product_table),
                format!("Lua plugin '{}': execute() failed", self.name),
            )
        });
        lua.remove_app_data::<CtxPtr>();
        drop(lua);
        result
    }

    fn clean(&self, product: &Product, verbose: bool) -> Result<usize> {
        if self.has_function("clean") {
            let existed_before = product.outputs.iter().filter(|o| o.exists()).count();
            let lua = self.lua.lock();
            let product_table = Self::product_to_lua(&lua, product)?;
            let clean_fn: LuaFunction = lua_context(
                lua.globals().get("clean"),
                format!("Lua plugin '{}': clean() not found", self.name),
            )?;
            lua_context(
                clean_fn.call::<()>(product_table),
                format!("Lua plugin '{}': clean() failed", self.name),
            )?;
            drop(lua);
            let exist_after = product.outputs.iter().filter(|o| o.exists()).count();
            Ok(existed_before.saturating_sub(exist_after))
        } else {
            clean_outputs(product, &product.processor, verbose)
        }
    }

    fn auto_detect(&self, file_index: &FileIndex) -> bool {
        let files = file_index.scan(&self.scan_config, true);
        if self.has_function("auto_detect") {
            let lua = self.lua.lock();
            let Ok(files_table) = lua.create_table() else {
                return !files.is_empty();
            };
            for (i, file) in files.iter().enumerate() {
                if files_table.set(i + 1, file.to_string_lossy().to_string()).is_err() {
                    return !files.is_empty();
                }
            }
            match lua.globals()
                .get::<LuaFunction>("auto_detect")
                .and_then(|f| f.call::<bool>(files_table))
            {
                Ok(detected) => detected,
                Err(e) => {
                    // The trait can't propagate errors; a broken plugin must
                    // not be silently treated as detected/undetected.
                    eprintln!("Warning: Lua plugin '{}': auto_detect() failed: {e}", self.name);
                    !files.is_empty()
                }
            }
        } else {
            !files.is_empty()
        }
    }

    fn required_tools(&self) -> Vec<String> {
        if self.has_function("required_tools") {
            let result = self.lua.lock().globals()
                .get::<LuaFunction>("required_tools")
                .and_then(|f| f.call::<LuaTable>(()))
                .and_then(|table| {
                    let mut tools = Vec::new();
                    for i in 1..=table.len()? {
                        let tool: String = table.get(i)?;
                        tools.push(tool);
                    }
                    Ok(tools)
                });
            match result {
                Ok(tools) => tools,
                Err(e) => {
                    // The trait can't propagate errors; an empty tool list
                    // would silently skip the tool pre-flight for this plugin.
                    eprintln!("Warning: Lua plugin '{}': required_tools() failed: {e}", self.name);
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        }
    }

    fn tool_version_commands(&self) -> Vec<(String, Vec<String>)> {
        self.required_tools()
            .into_iter()
            .map(|tool| (tool, vec!["--version".to_string()]))
            .collect()
    }
}
