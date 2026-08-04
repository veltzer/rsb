# Lua Plugins

RSConstruct supports custom processors written in Lua. Drop a `.lua` file in the `plugins/` directory and add a `[processor.NAME]` section in `rsconstruct.toml`. The plugin participates in discovery, execution, caching, cleaning, tool listing, and auto-detection just like a built-in processor.

## Quick Start

**1. Create the plugin file:**

```
plugins/eslint.lua
```

```lua
function description()
    return "Lint JavaScript/TypeScript with ESLint"
end

function required_tools()
    return {"eslint"}
end

function discover(project_root, config, files)
    local products = {}
    for _, file in ipairs(files) do
        local stub = rsconstruct.stub_path(file, "eslint")
        table.insert(products, {
            inputs = {file},
            outputs = {stub},
        })
    end
    return products
end

function execute(product)
    rsconstruct.run_command("eslint", {product.inputs[1]})
    rsconstruct.write_stub(product.outputs[1], "linted")
end
```

**2. Enable it in `rsconstruct.toml`:**

```toml
[processor.eslint]
src_dirs = ["src"]
src_extensions = [".js", ".ts"]
```

**3. Run it:**

```sh
rsconstruct build            # builds including the plugin
rsconstruct processors list   # shows the plugin
rsconstruct processors files  # shows files discovered by the plugin
```

## Lua API Contract

Each `.lua` file defines global functions. Three are required; the rest have sensible defaults.

### Required Functions

#### `description()`

Returns a human-readable string describing what the processor does. Called once when the plugin is loaded.

```lua
function description()
    return "Lint JavaScript files with ESLint"
end
```

#### `discover(project_root, config, files)`

Called during product discovery. Receives:

- `project_root` (string) — absolute path to the project root
- `config` (table) — the `[processor.NAME]` TOML section as a Lua table
- `files` (table) — list of absolute file paths matching the scan configuration

Must return a table of products. Each product is a table with `inputs` and `outputs` keys, both containing tables of absolute file paths.

```lua
function discover(project_root, config, files)
    local products = {}
    for _, file in ipairs(files) do
        local stub = rsconstruct.stub_path(file, "myplugin")
        table.insert(products, {
            inputs = {file},
            outputs = {stub},
        })
    end
    return products
end
```

#### `execute(product)`

Called to build a single product. Receives a table with `inputs` and `outputs` keys (both tables of absolute path strings). Must create the output files on success or error on failure.

```lua
function execute(product)
    rsconstruct.run_command("mytool", {product.inputs[1]})
    rsconstruct.write_stub(product.outputs[1], "done")
end
```

### Optional Functions

#### `clean(product)`

Called when running `rsconstruct clean outputs`. Receives the same product table as `execute()`. Default behavior: removes every file in `product.outputs` (file-only — no directory recursion).

```lua
function clean(product)
    for _, output in ipairs(product.outputs) do
        rsconstruct.remove_file(output)
    end
end
```

**Scope.** A Lua processor's `clean()` should remove only the files it produced. Do not recursively delete directories from a Lua plugin — that is a Creator-only contract, and Lua plugins are treated as Generators. If your tool genuinely produces a directory of unknown contents, declare an Explicit `[processor.explicit.*]` stanza with `output_dirs` instead of writing recursive deletes in Lua.

**Empty-directory sweep.** After your `clean()` returns, the orchestrator walks every parent directory of every path in `product.outputs` bottom-up and removes any directory that is now empty. You do not need to clean up empty parent directories yourself — and you should not, because your plugin doesn't know whether siblings of those paths were produced by other processors. The user can disable the sweep with `rsconstruct clean outputs --no-empty-dirs`.

See [`rsconstruct clean`](commands.md#rsconstruct-clean) for the full per-processor-type clean contract.

#### `auto_detect(files)`

Called to determine whether this processor is relevant for the project (when `auto_detect = true` in config). Receives the list of matching files. Default: returns `true` if the files list is non-empty.

```lua
function auto_detect(files)
    return #files > 0
end
```

#### `required_tools()`

Returns a table of external tool names required by this processor. Used by `rsconstruct tools list-configured` and `rsconstruct tools check`. Default: empty table.

```lua
function required_tools()
    return {"eslint", "node"}
end
```

#### Checkers: stub files or no outputs

A checker plugin chooses one of two output shapes in `discover()`:

**Option 1: with stub files** — return `outputs = {stub}` from `discover()`
and call `rsconstruct.write_stub()` in `execute()`. The stub file records
the pass on disk.

**Option 2: without stub files** — return `outputs = {}` from `discover()`
and don't write stubs in `execute()`. The cache entry itself serves as the
success record.

(An earlier version of this page documented a `processor_type()` hook; no
Rust code ever read it, and it has been removed.)

## The `rsconstruct` Global Table

Lua plugins have access to an `rsconstruct` global table with helper functions.

| Function | Description |
|---|---|
| `rsconstruct.stub_path(source, suffix)` | Compute the stub output path for a source file (relative to the project root). Maps `a/b/file.ext` to `out/suffix/a_b_file.ext.suffix`. |
| `rsconstruct.run_command(program, args)` | Run an external command. Errors if the command fails (non-zero exit). |
| `rsconstruct.run_command_cwd(program, args, cwd)` | Run an external command with a working directory. |
| `rsconstruct.write_stub(path, content)` | Write a stub file (creates parent directories as needed). |
| `rsconstruct.remove_file(path)` | Remove a file if it exists. No error if the file is missing. |
| `rsconstruct.file_exists(path)` | Returns `true` if the file exists. |
| `rsconstruct.read_file(path)` | Read a file and return its contents as a string. |
| `rsconstruct.path_join(parts)` | Join path components. Takes a table: `rsconstruct.path_join({"a", "b", "c"})` returns `"a/b/c"`. |
| `rsconstruct.log(message)` | Print a message prefixed with the plugin name. |

## Configuration

Plugins use the standard scan configuration fields. Any `[processor.NAME]` section in `rsconstruct.toml` is passed to the plugin's `discover()` function as the `config` table.

### Scan Configuration

These fields control which files are passed to `discover()`:

| Key | Type | Default | Description |
|---|---|---|---|
| `src_dirs` | string[] | `[""]` | Directory to scan (`""` = project root) |
| `src_extensions` | string[] | `[]` | File extensions to match |
| `src_exclude_dirs` | string[] | `[]` | Directory path segments to skip |
| `src_exclude_files` | string[] | `[]` | File names to skip |
| `src_exclude_paths` | string[] | `[]` | Paths relative to project root to skip |

### Custom Configuration

Any additional keys in the `[processor.NAME]` section are passed through to the Lua `config` table:

```toml
[processor.eslint]
src_dirs = ["src"]
src_extensions = [".js", ".ts"]
max_warnings = 0          # custom key, accessible as config.max_warnings in Lua
fix = false               # custom key, accessible as config.fix in Lua
```

```lua
function execute(product)
    local args = {product.inputs[1]}
    if config.max_warnings then
        table.insert(args, "--max-warnings")
        table.insert(args, tostring(config.max_warnings))
    end
    rsconstruct.run_command("eslint", args)
    rsconstruct.write_stub(product.outputs[1], "linted")
end
```

### Plugins Directory

The directory where RSConstruct looks for `.lua` files is configurable:

```toml
[plugins]
dir = "plugins"  # default
```

## Plugin Name Resolution

The plugin name is derived from the `.lua` filename (without extension). This name is used for:

- The `[processor.NAME]` config section
- The `[processor.NAME]` config section in `rsconstruct.toml`
- The `out/NAME/` stub directory
- Display in `rsconstruct processors list` and build output

A plugin name must not conflict with a built-in processor name (`tera`, `ruff`, `pylint`, `cc_single_file`, `cppcheck`, `shellcheck`, `zspell`, `make`). RSConstruct will error if a conflict is detected.

## Incremental Builds

Lua plugins participate in RSConstruct's incremental build system automatically:

- Products are identified by their inputs, outputs, and a config hash
- If none of the declared inputs have changed since the last build, the product is skipped
- If the `[processor.NAME]` config section changes, all products are rebuilt
- Outputs are cached and can be restored from cache

For correct incrementality, make sure `discover()` declares all files that affect the output. If your tool reads additional configuration files, include them in the `inputs` list.

## Examples

### Linter Without Stub Files (Recommended)

A checker that validates files without producing stub files. Success is recorded in the cache database.

```lua
function description()
    return "Lint YAML files with yamllint"
end

function processor_type()
    return "checker"
end

function required_tools()
    return {"yamllint"}
end

function discover(project_root, config, files)
    local products = {}
    for _, file in ipairs(files) do
        table.insert(products, {
            inputs = {file},
            outputs = {},  -- No output files
        })
    end
    return products
end

function execute(product)
    rsconstruct.run_command("yamllint", {"-s", product.inputs[1]})
    -- No stub to write; cache entry = success
end

function clean(product)
    -- Nothing to clean
end
```

```toml
[processor.yamllint]
src_extensions = [".yml", ".yaml"]
```

### Stub-Based Linter (Legacy)

A linter that creates stub files. Use this if you need the stub file for some reason.

```lua
function description()
    return "Lint YAML files with yamllint"
end

function processor_type()
    return "checker"
end

function required_tools()
    return {"yamllint"}
end

function discover(project_root, config, files)
    local products = {}
    for _, file in ipairs(files) do
        table.insert(products, {
            inputs = {file},
            outputs = {rsconstruct.stub_path(file, "yamllint")},
        })
    end
    return products
end

function execute(product)
    rsconstruct.run_command("yamllint", {"-s", product.inputs[1]})
    rsconstruct.write_stub(product.outputs[1], "linted")
end
```

```toml
[processor.yamllint]
src_extensions = [".yml", ".yaml"]
```

### File Transformer (Generator)

A plugin that transforms input files into output files (not stubs). This is a "generator" processor.

```lua
function description()
    return "Compile Sass to CSS"
end

function processor_type()
    return "generator"
end

function required_tools()
    return {"sass"}
end

function discover(project_root, config, files)
    local products = {}
    for _, file in ipairs(files) do
        local out = file:gsub("%.scss$", ".css"):gsub("^" .. project_root .. "/src/", project_root .. "/out/sass/")
        table.insert(products, {
            inputs = {file},
            outputs = {out},
        })
    end
    return products
end

function execute(product)
    rsconstruct.run_command("sass", {product.inputs[1], product.outputs[1]})
end
```

```toml
[processor.sass]
src_dirs = ["src"]
src_extensions = [".scss"]
```
