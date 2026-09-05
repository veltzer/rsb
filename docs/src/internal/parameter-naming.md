# Parameter Naming Conventions

This document establishes the canonical names for configuration parameters across
all processors, and the reasoning behind each name. Use this as the reference when
adding new processors or renaming existing ones.

## Taxonomy

Parameters fall into four categories:

| Category | Purpose |
|----------|---------|
| **Source discovery** | Which files are the primary targets to process |
| **Dependency tracking** | Which additional files affect the checksum / trigger rebuilds |
| **Tool configuration** | What command/tool to run and how |
| **Execution control** | Batching, parallelism, output location |

---

## Source Discovery Parameters

These parameters determine which files are the primary inputs — the files that
get processed, linted, or transformed.

| Parameter | Type | Description |
|-----------|------|-------------|
| `src_dirs` | string[] | Directories to scan recursively for source files. |
| `src_extensions` | string[] | File extensions to match during scanning (e.g. `[".py", ".pyi"]`). |
| `src_exclude_dirs` | string[] | Directory path segments to skip during scanning. |
| `src_exclude_files` | string[] | File names to skip during scanning. |
| `src_exclude_paths` | string[] | Exact relative paths to skip during scanning. |
| `src_files` | string[] | Explicit list of source files to process. When set, bypasses `src_dirs`, `src_extensions`, and all exclude filters entirely. |

### `src_files` vs scanning

`src_dirs` + `src_extensions` is the default discovery mechanism — the processor
walks directories and finds matching files automatically.

`src_files` is for when you know exactly which files you want processed and
don't want any scanning. Setting `src_files` disables all scan-based
discovery for that processor instance.

---

## Dependency Tracking Parameters

These parameters declare files that the processor depends on but does not
process directly. A change to any of these files invalidates the cache and
triggers a rebuild, but the files are not passed as arguments to the tool.

| Parameter | Type | Description |
|-----------|------|-------------|
| `dep_inputs` | string[] | Explicit dependency files (e.g. config files, schema files). Globs are supported. Fails if a listed file does not exist. |
| `dep_auto` | string[] | Like `dep_inputs`, but a processor's default list is skipped where the files do not exist. Used for optional config files (e.g. `.pylintrc`, `pyproject.toml`). A list written in the config replaces the default and is checked like `dep_inputs` unless `[build] allow_missing_dep_auto` is set. |

### Why two parameters?

`dep_inputs` is strict — it errors if a file is missing, which catches
mistakes in configuration. `dep_auto` is where a processor declares the
well-known config files it honours when present: a project without
`.pylintrc` is fine, so those defaults are lenient. The leniency stops at
the processor's defaults. Once a user writes a `dep_auto` list, each entry
is a claim about the project, and a missing one is a typo or a stale copy
of a shared config — a dependency that tracks nothing — so it fails at
config load like a missing `dep_inputs` entry would. `[build]
allow_missing_dep_auto = true` restores skip-if-absent for listed entries
too.

---

## Tool Configuration Parameters

`command` and `args` always appear together. Every processor that has `command`
must also have `args`. They are treated as a unit: both participate in the
config checksum (computed from each processor's `checksum_fields()`), so
changing either the command or any argument invalidates the cache and triggers
a rebuild.

| Parameter | Type | Description |
|-----------|------|-------------|
| `command` | string | The executable to run. Required when the processor is active. If the value is a path to a local file, its content checksum is also tracked as a dependency. |
| `args` | string[] | Arguments passed to the command before file paths. Always present alongside `command`. Both `command` and `args` values are included in the config checksum. |

### `command` dependency tracking

For the `script` and `generator` processors, if `command` points to a file that
exists on disk (e.g. `command = "scripts/my_linter.sh"`), rsconstruct
automatically adds it as an input dependency. This means that if the script
itself changes, all affected products are rebuilt. System tools (e.g. `bash`,
`python3`) are not files in the project and are not tracked.

---

## Execution Control Parameters

| Parameter | Type | Description |
|-----------|------|-------------|
| `batch` | bool | When `true`, pass all files to the command in a single invocation. When `false`, invoke once per file. Default: `true` for most processors. |
| `max_jobs` | int | Maximum parallel jobs for this processor. Overrides the global `--jobs` flag. |
| `output_dir` | string | Directory where output files are written (generator processors). |
| `output_extension` | string | File extension for generated output files. |
