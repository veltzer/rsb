# Configuration

RSConstruct is configured via an `rsconstruct.toml` file in the project root.
An optional `rsconstruct.local.toml` overlay, when present, is merged over the
main file at load time — see [Local overlay](#local-overlay-rsconstructlocaltoml).

## Full reference

```toml
[build]
parallel = 1          # Number of parallel jobs (1 = sequential, 0 = auto-detect CPU cores)
                      # Also settable via RSCONSTRUCT_THREADS env var (CLI -j takes precedence)
batch_size = 0        # Max files per batch for batch-capable processors (0 = no limit)
output_dir = "out"    # Global output directory prefix for generator processors
skip_missing_src_dirs = false  # true: missing src_dirs entries skip instead of fail

# Declare processors by adding [processor.NAME] sections.
# Only declared processors run — no processors are enabled by default.
# Use `rsconstruct smart auto` to auto-detect and add relevant processors.

[processor.ruff]
# args = []

[processor.pylint]
# args = ["--disable=C0114"]

[processor.cc_single_file]
# cc = "gcc"
# cflags = ["-Wall", "-O2"]

[vars]
my_excludes = ["/vendor/", "/third_party/"]  # Define variables for reuse with ${var_name}

[cache]
restore_method = "auto"  # auto (default: copy in CI, hardlink otherwise), hardlink, or copy
compression = false      # Compress cached objects with zstd (requires restore_method = "copy")
remote = "s3://my-bucket/rsconstruct-cache"  # Optional: remote cache URL
remote_push = true       # Push local builds to remote (default: true)
remote_pull = true       # Pull from remote cache on cache miss (default: true)
mtime_check = true       # Use mtime pre-check to skip unchanged file checksums (default: true)

[analyzer]
auto_detect = true
enabled = ["cpp", "python"]

[graph]
viewer = "google-chrome"  # Command to open graph files (default: platform-specific)

[plugins]
dir = "plugins"  # Directory containing .lua processor plugins

[completions]
shells = ["bash"]

[dependencies]
pip = ["pyyaml", "jinja2"]    # Python packages
npm = ["eslint", "prettier"]  # Node.js packages
gem = ["mdl"]                 # Ruby gems
system = ["pandoc", "graphviz"]  # System packages (checked but not auto-installed)
```

Per-processor configuration is documented on each processor's page under [Processors](processors.md).
Lua plugin configuration is documented under [Lua Plugins](plugins.md).

## Processor instances

Processors are declared by adding a `[processor.NAME]` section to `rsconstruct.toml`. An empty section enables the processor with default settings:

```toml
[processor.pylint]
```

Customize with config fields:

```toml
[processor.pylint]
args = ["--disable=C0114,C0116"]
src_dirs = ["src"]
```

Remove the section to disable the processor.

### Multiple instances

Run the same processor multiple times with different configurations by adding named sub-sections:

```toml
[processor.pylint.core]
src_dirs = ["src/core"]
args = ["--disable=C0114"]

[processor.pylint.tests]
src_dirs = ["tests"]
args = ["--disable=C0114,C0116"]
```

Each instance runs independently with its own config and cache.

You cannot mix single-instance and multi-instance formats for the same processor type — use either `[processor.pylint]` or `[processor.pylint.NAME]`, not both.

#### Instance naming

A single instance declared as `[processor.pylint]` has the instance name `pylint`. Named instances declared as `[processor.pylint.core]` and `[processor.pylint.tests]` have instance names `pylint.core` and `pylint.tests`.

The instance name is used everywhere a processor is identified:

- **Build output and progress**: `[pylint.core] src/core/main.py`
- **Error messages**: `Error: [pylint.tests] tests/test_foo.py: ...`
- **Build statistics**: each instance reports its own file counts and durations
- **Cache keys**: instances have separate caches, so changing one config does not invalidate the other
- **Output directories**: generator processors default to `out/{instance_name}` (e.g., `out/marp.slides` and `out/marp.docs` for two marp instances), ensuring outputs do not collide
- **The `--processors` filter**: use the full instance name, e.g., `rsconstruct build -p pylint.core`

For single instances, the instance name equals the processor type name (e.g., `pylint`), so there is no visible difference from previous behavior.

### Auto-detection

Run `rsconstruct smart auto` to scan the project and automatically add `[processor.NAME]` sections for all processors whose files are detected and whose tools are installed. It does not remove existing sections.

## Variable substitution

Define variables in a `[vars]` section and reference them using `${var_name}` syntax:

```toml
[vars]
kernel_excludes = ["/kernel/", "/kernel_standalone/", "/examples_standalone/"]

[processor.cppcheck]
src_exclude_dirs = "${kernel_excludes}"

[processor.cc_single_file]
src_exclude_dirs = "${kernel_excludes}"
```

Variables are substituted before TOML parsing. The `"${var_name}"` (including quotes) is replaced with the TOML-serialized value, preserving types (arrays stay arrays, strings stay strings). Undefined variable references produce an error.

## Local overlay (`rsconstruct.local.toml`)

If a file named `rsconstruct.local.toml` exists next to `rsconstruct.toml`, it
is loaded automatically and deep-merged over the main config. This lets many
repos share one canonical, identical `rsconstruct.toml` while keeping
repo-specific configuration — `[dependencies]` lists, `[processor.explicit.*]`
generators, exclude lists, one-off workarounds — in the local file.

Merge semantics:

- **Tables merge recursively.** A local `[processor.mypy]` with only
  `batch = false` adds that one key to the main file's mypy section; the rest
  of the section is untouched.
- **Arrays and scalars replace wholesale.** If the local file sets
  `src_dirs`, that exact list is used — values are never concatenated, so the
  local file can also *remove* entries by setting a shorter list.
- **Local-only sections are added.** The overlay can declare whole processors,
  analyzers, or global sections the main file doesn't have.

Rules and caveats:

- The overlay only extends a main config: `rsconstruct.local.toml` without an
  `rsconstruct.toml` is an error.
- `[vars]` substitution is per-file — each file's `${...}` references resolve
  against its own `[vars]` section only.
- The merged result is validated as one config, so unknown fields or type
  errors in the local file are reported the same way as in the main file.
- `rsconstruct processors config <iname>` shows the merged values.
- In watch mode, both files are watched for changes.

Combined with [`skip_missing_src_dirs`](#build), this supports the
shared-config pattern: one identical `rsconstruct.toml` distributed across
many repos (each processor only activates where its directories exist), plus a
small optional `rsconstruct.local.toml` per repo for what's genuinely
repo-specific.

## Section details

### `[build]`

| Key | Type | Default | Description |
|---|---|---|---|
| `parallel` | integer | `1` | Number of parallel jobs. `1` = sequential, `0` = auto-detect CPU cores. Can also be set via the `RSCONSTRUCT_THREADS` environment variable (CLI `-j` takes precedence). |
| `batch_size` | integer | `0` | Maximum files per batch for batch-capable processors. `0` = no limit (all files in one batch). To disable batching, pass `--batch-size -1` on the CLI or set `batch = false` on individual processors. |
| `output_dir` | string | `"out"` | Global output directory prefix. Processor `output_dir` defaults that start with `out/` are remapped to use this prefix (e.g., setting `"build"` changes `out/marp` to `build/marp`). Individual processors can still override their `output_dir` explicitly. |
| `skip_missing_src_dirs` | boolean | `false` | When `true`, a `src_dirs` entry that doesn't exist on disk deactivates the processor's scan of that directory instead of failing the build. Lets one shared config file serve repos with different layouts: a processor whose directories are all absent simply matches no files. Skipped entries are listed when running with `--phases`. Leave `false` so a missing directory is caught as a typo. |

The `output_dir` prefix is purely a layout choice — `rsconstruct clean outputs` does not special-case it. Cleanup is driven by per-product `outputs` and `output_dirs` declarations, then a generic empty-directory sweep walks parents bottom-up. See [Clean behavior](processors.md#clean-behavior) and [`rsconstruct clean`](commands.md#rsconstruct-clean) for details.

### `[processor.NAME]`

Each `[processor.NAME]` section declares a processor instance. The section name must match a builtin processor type (e.g., `ruff`, `pylint`, `cc_single_file`) or a [Lua plugin](plugins.md) name.

Common fields available to all processors:

| Key | Type | Default | Description |
|---|---|---|---|
| `enabled` | boolean | `true` | Set to `false` to disable this processor without removing the stanza. Accepted on every processor. |
| `cache` | boolean | `true` | Whether to cache this processor's outputs. Set to `false` to always rebuild and never store results. Accepted on every processor. |
| `args` | array of strings | `[]` | Extra command-line arguments passed to the tool. |
| `dep_inputs` | array of strings | `[]` | Additional input files that trigger rebuild when changed. |
| `dep_auto` | array of strings | varies | Config files auto-detected as inputs (e.g., `.pylintrc`). |
| `batch` | boolean | `true` | Whether to batch multiple files into a single tool invocation. Note: in fail-fast mode (default), chunk size is 1 regardless of this setting — batch mode only groups files with `--keep-going` or `--batch-size`. For external tools, a batch failure marks all products in the chunk as failed. Internal processors (`i`-prefixed) return per-file results, so partial failure is handled correctly. |
| `max_jobs` | integer | none | Maximum concurrent jobs for this processor. When set, limits how many instances of this processor run in parallel, regardless of the global `-j` setting. Useful for heavyweight processors (e.g., `marp` spawns Chromium). Omit to use the global parallelism. |
| `src_dirs` | array of strings | varies | Directories to scan for source files. **Required** for most processors (defaults to `[]`). Processors with a specific default (e.g., `tera` defaults to `"tera.templates"`, `cc_single_file` defaults to `"src"`) do not require this. Not required when `src_files` is set. **Every entry must exist on disk** (or be the declared output target of an upstream processor) — a missing directory fails the build with `src_dirs entry 'X' does not exist or is not a directory`, unless `[build] skip_missing_src_dirs = true` is set, in which case the missing entry is skipped and the processor simply matches no files there. Use `rsconstruct processors defconfig <name>` to see a processor's defaults. |
| `src_extensions` | array of strings | varies | File extensions to match. |
| `src_exclude_dirs` | array of strings | varies | Directory path segments to exclude from scanning. |
| `src_exclude_files` | array of strings | `[]` | File names to exclude. |
| `src_exclude_paths` | array of strings | `[]` | Paths (relative to project root) to exclude. |
| `src_files` | array of strings | `[]` | When non-empty, only these exact paths are matched — `src_dirs`, `src_extensions`, and exclude filters are bypassed. Useful for processors that operate on specific files rather than scanning directories. |

Processor-specific fields are documented on each processor's page under [Processors](processors.md).

### `[cache]`

| Key | Type | Default | Description |
|---|---|---|---|
| `restore_method` | string | `"auto"` | How to restore cached outputs. `"auto"` (default) uses `"copy"` in CI environments (`CI=true`) and `"hardlink"` otherwise. `"hardlink"` is faster but requires same filesystem; `"copy"` works everywhere. |
| `compression` | boolean | `false` | Compress cached objects with zstd. Incompatible with `restore_method = "hardlink"` — requires `"copy"`. |
| `remote` | string | none | Remote cache URL. See [Remote Caching](remote-caching.md). |
| `remote_push` | boolean | `true` | Push locally built artifacts to remote cache. |
| `remote_pull` | boolean | `true` | Pull from remote cache on local cache miss. |
| `mtime_check` | boolean | `true` | Persist file checksums across builds using an mtime database. Set to `false` in CI/CD environments where the cache won't survive the build and the write overhead isn't worth it. Can also be disabled via `--no-mtime-cache` flag. See [Checksum Cache](internal/checksum-cache.md). |

### `[analyzer]`

| Key | Type | Default | Description |
|---|---|---|---|
| `auto_detect` | boolean | `true` | When `true`, only run enabled analyzers that auto-detect relevant files. |
| `enabled` | array of strings | `["cpp", "python"]` | List of dependency analyzers to enable. |

### `[graph]`

| Key | Type | Default | Description |
|---|---|---|---|
| `viewer` | string | platform-specific | Command to open graph files |

### `[plugins]`

| Key | Type | Default | Description |
|---|---|---|---|
| `dir` | string | `"plugins"` | Directory containing `.lua` processor plugins |

### `[completions]`

| Key | Type | Default | Description |
|---|---|---|---|
| `shells` | array | `["bash"]` | Shells to generate completions for |

### `[dependencies]`

Declare project dependencies by package manager. Used by `rsconstruct doctor` to verify availability and `rsconstruct tools install-deps` to install missing packages.

| Key | Type | Default | Description |
|---|---|---|---|
| `pip` | array of strings | `[]` | Python packages to install via `pip install`. Supports version specifiers (e.g., `"ruff>=0.4"`). |
| `npm` | array of strings | `[]` | Node.js packages to install via `npm install`. |
| `gem` | array of strings | `[]` | Ruby gems to install via `gem install`. |
| `system` | array of strings | `[]` | System packages installed via the detected package manager (`apt-get`, `dnf`, `pacman`, or `brew`). |

#### Install order

`rsconstruct tools install-deps` always installs in this fixed order:

1. **`system`** — OS packages (apt, dnf, pacman, brew)
2. **`pip`** — Python packages
3. **`npm`** — Node.js packages
4. **`gem`** — Ruby gems

This order is deliberate and must not be changed. Language-level packages frequently build native extensions that link against system libraries at install time. For example, installing `manim` via pip pulls in `manimpango`, which compiles a C extension and uses `pkg-config` to find `pangocairo` — so `libpango1.0-dev` must already be on the system before `pip install` runs. Running `pip` (or `gem`, or `npm`) before `system` causes wheel/extension builds to fail with messages like `Package 'pangocairo' was not found`.

The keys inside `[dependencies]` may appear in any order in `rsconstruct.toml`; the install order is enforced by `install-deps` regardless.

#### `eatmydata` wrapping

When [`eatmydata`](https://www.flamingspork.com/projects/libeatmydata/) is installed on the system *and* `CI=true` is in the environment, both `rsconstruct tools install` and `rsconstruct tools install-deps` wrap their `apt`/`dnf`/`pacman` invocations with it. `eatmydata` no-op's `fsync()` for the wrapped process, which speeds up package installs by 3–10×.

The trade-off is loss-on-power-cut: any package files written during the install are not flushed to disk, so a power loss mid-install can leave the package database inconsistent. That's fine on transient CI hosts and wrong on developer workstations — hence the `CI=true` gate.

The wrap inserts `eatmydata` *after* `sudo` so the `LD_PRELOAD` applies to the package manager, not to `sudo` itself: e.g. `sudo eatmydata apt-get install -y …`.

If `eatmydata` is not installed, the commands run unwrapped — no error, no warning.

##### Controlling the wrap

| To...                                       | Do this                                              |
| ------------------------------------------- | ---------------------------------------------------- |
| Use the wrap (the CI default)               | Set `CI=true` and have eatmydata installed           |
| Skip the wrap in CI                         | Unset `CI` (or set it to anything other than `true`) |
| Use the wrap outside CI                     | Run with `CI=true rsconstruct tools install-deps`    |
| Skip the wrap for a single invocation       | Pass `--no-eatmydata`                                |

The CLI flag `--no-eatmydata` always wins. Otherwise the policy is driven entirely by `CI=true`. There is no `rsconstruct.toml` field for this — the env var is the knob.

The mechanism is a [post-config phase hook](processors.md): `eatmydata_ci_default` runs after config load and flips the in-memory `dependencies.eatmydata` flag when `CI=true`. List it with `rsconstruct phases hooks`.

##### What's never wrapped

`brew` is never wrapped (eatmydata is Linux-only). `pip`, `npm`, `cargo`, and `gem` are never wrapped either — those don't fsync excessively, so the wrap adds nothing.
