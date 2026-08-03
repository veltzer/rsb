# Architecture

This page describes RSConstruct's internal design for contributors and those interested in how the tool works.

## Core concepts

### Processors

Processors implement the `Processor` trait (`src/processors/mod.rs`). Each processor:

1. **Auto-detects** whether it is relevant for the current project
2. Scans the project for source files matching its conventions
3. Creates **products** describing what to build
4. Executes the build for each product

Run `rsconstruct processors list` to see all available processors and their auto-detection results.

### Auto-detection

Every processor implements `auto_detect()`, which returns `true` if the processor appears relevant for the current project based on filesystem heuristics. This allows RSConstruct to guess which processors a project needs without requiring manual configuration.

Only two `Processor` methods are required — `scan_config()` and
`execute(ctx, product)`. Everything else has a default implementation driven
by the processor's `StandardConfig`. The main methods:

| Method | Purpose |
|---|---|
| `auto_detect(&FileIndex) -> bool` | Return `true` if the project looks like it needs this processor |
| `discover(&mut BuildGraph, &FileIndex, instance_name)` | Query the file index and add products to the build graph |
| `execute(&BuildContext, &Product)` | Build a single product |
| `clean(&Product, verbose) -> Result<usize>` | Remove a product's outputs, returning how many files were removed |

Batch execution (`execute_batch`), fixing (`fix`, `fix_batch`,
`config_has_fix`), and tool declaration (`required_tools`,
`tool_version_commands`) round out the trait.

Both `auto_detect` and `discover` receive a `&FileIndex` — a pre-built index of all non-ignored files in the project (see [File indexing](#file-indexing) below).

Detection heuristics are per-processor filesystem checks (e.g. `ruff`
detects when the project contains `.py` files, `cargo` when it contains
`Cargo.toml`). The authoritative, always-current list is
`rsconstruct processors list`, which shows every processor together with its
auto-detection result for the current project.

### Products

A product represents a single build unit with:

- **Inputs** — source files that the product depends on
- **Outputs** — files that the product generates
- **Output directory** (optional) — for creators, the directory whose entire contents are cached and restored as a unit

### BuildGraph

The `BuildGraph` manages dependencies between products. It performs a topological sort to determine the correct build order, ensuring that dependencies are built before the products that depend on them.

### Executor

The executor runs products in dependency order. It supports:

- Sequential execution (default)
- Parallel execution of independent products (with `-j` flag)
- Dry-run mode (show what would be built)
- Keep-going mode (continue after errors)
- Batch execution (group multiple products into one tool invocation)

### Incremental rebuild after partial failure

Each product is cached independently after successful execution. If a build is
interrupted or fails partway through, the next run only rebuilds products that
don't have valid cache entries:

- **Non-batch mode**: Each product executes and is cached individually. If
  the build stops after 400 of 800 products, the next run skips the 400
  cached successes and rebuilds the remaining 400. Products go down this
  path when batching is disabled (`--batch-size -1`), when the processor
  doesn't support batching, or when fewer than two of its products need
  rebuilding.

- **Batch mode with external tools** (the default for batch-capable
  processors): The external tool receives all files in the batch in one
  invocation — with no `--batch-size` limit set, that is every rebuilding
  product of the processor in the level. If the tool exits with an error,
  all products in that batch are marked failed — there is no way to
  determine which outputs are valid from a single exit code. On the next
  run, all products from the failed batch are rebuilt. Use
  `--batch-size N` to bound the blast radius, or `--batch-size -1` for
  per-product execution and caching.

- **Batch mode with internal processors** (e.g., `imarkdown2html`, `isass`, `ipdfunite`):
  These process files sequentially in-process and return per-file results, so
  partial failure is handled correctly even in batch mode — only the failed
  products are rebuilt.

## Interrupt handling

External subprocess execution goes through the runner family in
`src/processors/mod.rs` — `run_command()`, `run_command_capture()`,
`run_command_with_timeout()`, `run_command_with_stdin()` — which share one
inner implementation. It spawns children via `tokio::process::Command`
with `kill_on_drop(true)` and then awaits a biased `tokio::select!` racing
the child's exit against a `tokio::sync::watch` interrupt receiver obtained
from `BuildContext::interrupt_receiver()` (plus an optional timeout arm).

Routing through it is also what gives a call site `log_command()`
(`--show-child-processes`) and, in debug builds, the declared-tools
assertion that a processor only runs tools its `required_tools()` names.
Hand-rolled spawns silently opt out of all four, which is why they are held
to exactly two exceptions:

- **Tool installation** (`tools::run`, the binary installer) inherits
  the terminal so `sudo` can prompt for a password and apt/dnf can render
  progress. Capturing would hang on the prompt.
- **Opening a viewer** (`Builder::open_file`) launches a detached process
  that must outlive rsconstruct — `kill_on_drop` and waiting are both
  exactly wrong.

Both are commented as such at the call site. Anything else that spawns a
child is a bug.

A command that needs to feed stdin uses `run_command_with_stdin()`, which
pumps the write concurrently with draining stdout/stderr. Writing all of
stdin first deadlocks as soon as the child fills its output pipe — this is
why the aspell checker used to carry a hand-rolled spawn with a writer
thread.

The Ctrl+C handler — a dedicated thread in `main.rs` awaiting
`tokio::signal::ctrl_c()` — calls `BuildContext::interrupt()`, which sets an
`AtomicBool` and broadcasts on the watch channel. Every waiting subprocess
wakes immediately (there is no polling interval) and its child is killed on
drop. The executor also consults `is_interrupted()` between products and
levels to stop scheduling new work. A second Ctrl+C force-exits with
status 130.

## File indexing

RSConstruct walks the project tree once at startup and builds a `FileIndex` — a sorted list of all non-ignored files. The walk is performed by the `ignore` crate (`ignore::WalkBuilder`), which natively handles:

- `.gitignore` — standard git ignore rules, including nested `.gitignore` files and negation patterns
- `.rsconstructignore` — project-specific ignore patterns using the same glob syntax as `.gitignore`

Processors never walk the filesystem themselves. Instead, `auto_detect` and `discover` receive a `&FileIndex` and query it with their scan configuration (src_extensions, exclude directories, exclude files). This replaces the previous design where each processor performed its own recursive walk.

## Build pipeline

This is the core algorithm — every `rsconstruct build` follows these phases
in order. Use `--phases` to see timing for each phase.

### Phase 1: File indexing

The project tree is walked once to build the `FileIndex` — a sorted list of
all non-ignored files. This is the only filesystem walk; all subsequent file
lookups go through the index. See [File indexing](#file-indexing) below.

### Phase 2: Discovery (fixed-point loop)

Each enabled processor queries the file index and adds products to the
`BuildGraph`. Discovery runs in a **fixed-point loop** to handle
cross-processor dependencies:

```
file_index = walk filesystem
loop (max 10 passes):
    for each processor:
        processor.discover(graph, file_index)
    if no new products were added → break
    collect outputs from new products
    inject them as virtual files into file_index
    if no new virtual files were injected → break
```

On each pass, processors may re-declare existing products (silently
deduplicated) or discover new products whose inputs are virtual files from
upstream generators. The loop converges when a full pass adds nothing new.
Most projects converge in 1 pass; projects with generator → checker/generator
chains converge in 2.

See [Cross-Processor Dependencies](cross-processor-dependencies.md) for
details on deduplication and the virtual file mechanism.

### Phase 3: Dependency analysis

Dependency analyzers (e.g., the C/C++ header scanner) run against the graph
to add additional input edges. For example, if `main.c` includes `util.h`,
the analyzer adds `util.h` as an input to the `main.c` product. Results are
cached in `deps.redb` for incremental builds.

### Phase 4: Tool version hashing

For each processor with a tool lock entry (`rsconstruct tools lock`), the
locked tool version hash is appended to the product's config hash. This
ensures that upgrading a tool (e.g., `ruff` 0.4 → 0.5) triggers rebuilds
even if source files haven't changed.

## Tool detection

The `TOOLS` registry in `src/tools.rs` lists every external tool
rsconstruct knows about. Each entry's `name` is the **detection key**: it is
passed directly to `which::which` by `builder/tools.rs` and `tool_lock.rs`, so
it decides whether `rsconstruct tools list` reports `installed` or `missing`,
and which binary `rsconstruct tools lock` records a version for.

**Registry names must be bare binary names, never paths.** `which` switches
behaviour on the presence of a path separator:

| Name form | Resolution | Depends on cwd? |
|---|---|---|
| `mdl` | searched on `$PATH` | no |
| `gems/bin/mdl` | relative to the current working directory | **yes** |
| `./gems/bin/mdl` | relative to the current working directory | **yes** |

A path-shaped entry therefore resolves only when rsconstruct is invoked from
the one directory that has that subtree beneath it, and reports `missing`
everywhere else — including for a user who has the tool installed and on
`$PATH` at, say, `~/install/gems/bin/mdl`. The registry once carried
`gems/bin/mdl` and `node_modules/.bin/markdownlint` alongside correct bare
`mdl` / `markdownlint` entries; both path forms were removed.

In both modes `which` also requires the executable bit — a file that exists but
is not executable reads as `missing`.

To pin a project-local binary, set the per-processor `command` config field
(`src/config/processor_configs.rs`), which is the value actually executed:

```toml
[markdownlint]
command = "node_modules/.bin/markdownlint"
```

The registry `name` stays bare so detection works from any working directory;
`command` is where a vendored path belongs.

### Phase 5: Dependency resolution

`resolve_dependencies()` scans the graph for products whose inputs match
other products' outputs. When found, it creates a dependency edge — the
producer must complete before the consumer can start. This is how
cross-processor ordering works automatically (e.g., pandoc runs before the
explicit site generator because pandoc's HTML outputs are the site
generator's inputs).

After resolution, the graph is topologically sorted to produce the execution
order.

### Phase 6: Classify

Each product is classified as one of:

- **Skip (up-to-date)** — input checksum matches the cache entry and all
  outputs exist on disk. No work needed.
- **Restore** — input checksum matches a cache entry but outputs are missing
  (e.g., after `rsconstruct clean`). Outputs are restored from cache via
  hardlink or copy.
- **Build (stale)** — input checksum doesn't match any cache entry. The
  product must be rebuilt.

Input checksums are computed by hashing all input files (SHA-256). The mtime
pre-check (`mtime_check = true`, default) skips rehashing files whose mtime
hasn't changed since the last build.

### Phase 7: Execute

Products are executed in topological order, respecting dependency edges.
Independent products at the same dependency level run in parallel (controlled
by `-j` / `RSCONSTRUCT_THREADS`). Batch-capable processors group their
products into a single tool invocation.

**Batch chunk sizing:** By default a batch group is sent as one chunk — all
of a batch-capable processor's rebuilding products in the level go to the
tool in a single invocation. With `--batch-size N`, chunks are limited to N
products. `--batch-size -1` disables batching entirely, giving per-product
execution and caching — the best incremental recovery after partial failure,
at the cost of one tool invocation per file. In fail-fast mode (no
`--keep-going`), a failing chunk stops later chunks from being dispatched.

For each product:
1. Compute input checksum (if not already done in classify)
2. Check cache — skip or restore if possible
3. Execute the processor's command
4. On success: store outputs in the cache (content-addressed under
   `.rsconstruct/objects/`)
5. On failure: report error (or continue if `--keep-going`)

## Processor source layout

All processor code lives under `src/processors/`. The folder structure mirrors processor type:

```
src/processors/
├── mod.rs          # Processor trait, shared helpers (run_command, run_checker,
│                   # SimpleChecker, SimpleGenerator, ProcessorBase, …)
├── checkers/       # One file per checker (ruff.rs, pylint.rs, cppcheck.rs, …)
│   └── mod.rs      # Re-exports
├── generators/     # One file per generator (generator.rs, marp.rs, sass.rs, tags.rs, …)
│   └── mod.rs      # Shared helpers: find_templates, output_path, discover_single_format, …
├── creators/       # One file per creator (cargo.rs, cc.rs, gem.rs, jekyll.rs,
│   │               # mdbook.rs, npm.rs, pip.rs, sphinx.rs)
│   ├── mod.rs      # Re-exports
│   └── creator.rs  # Generic creator processor
├── explicit/       # Explicit processor (user-defined command with declared outputs)
│   ├── mod.rs
│   └── explicit.rs
└── lua/            # Lua plugin host
    ├── mod.rs
    └── lua_processor.rs
```

### Conventions

- **Every file in `src/processors/` is a real single processor** — no utility-only files anywhere in the tree. Shared helpers live in `mod.rs` or `generators/mod.rs`; processor-specific data tables (e.g. the requirements generator's stdlib list) live in the processor's own file. The tool registry (`src/tools.rs`) and build statistics (`src/stats.rs`) live at the crate root for this reason.
- **Checkers** use `SimpleChecker` (data-driven, no boilerplate) or implement `Processor` directly for checkers with custom discovery logic (e.g., `clippy`, `script`).
- **Generators** use `SimpleGenerator` (data-driven with a custom `execute_fn`) or `GeneratorProcessor` for the generic pass-through generator.
- **Creators** use `CreatorProcessor` for the generic case, or their own struct for creators with special discovery (cargo profiles, npm siblings, etc.).
- **Explicit** is a singleton processor type with its own folder because it is neither a checker nor a generator.
- **Lua** is the only processor type that hosts external scripts rather than wrapping a fixed external tool. It has its own folder because it carries significant runtime state (the Lua VM).
- All processors self-register via `inventory::submit!` at the bottom of their file — no central registry table to update.

## Determinism

Build order is deterministic:

- File discovery is sorted
- Processor iteration order is sorted
- Topological sort produces a stable ordering

This ensures that the same project always builds in the same order, regardless of filesystem ordering.

## Caching

See [Cache System](cache.md) for full details on cache keys, storage format, rebuild classification, and per-processor caching behavior.

## Subprocess execution

RSConstruct uses two internal functions to run external commands:

- **`run_command()`** — by default captures stdout/stderr via OS pipes and only prints output on failure (quiet mode). Use `--show-output` flag to show all tool output. Use for compilers, linters, and any command where errors should be shown.

- **`run_command_capture()`** — always captures stdout/stderr via pipes. Use only when you need to parse the output (dependency analysis, version checks, Python config loading). Returns the output for processing.

### Parallel safety

When running with `-j`, each thread spawns its own subprocess. Each subprocess gets its own OS-level pipes for stdout/stderr, so there is no interleaving of output between concurrent tools. On failure, the captured output for that specific tool is printed atomically. This design requires no shared buffers or cross-thread output coordination.

## Path handling

**All paths are relative to project root.** RSConstruct assumes it is run from the project root directory (where `rsconstruct.toml` lives).

### Internal paths (always relative)
- `Product.inputs` and `Product.outputs` — stored as relative paths
- `FileIndex` — returns relative paths from `scan()` and `query()`
- Cache keys (`Product.cache_key()`) — use relative paths, enabling cache sharing across different checkout locations
- Cache entries (`CacheEntry.outputs[].path`) — stored as relative paths

### Processor execution
- Processors pass relative paths directly to external tools
- Processors set `cmd.current_dir(project_root)` to ensure tools resolve paths correctly
- `fs::read()`, `fs::write()`, etc. work directly with relative paths since cwd is project root

### Exception: Processors requiring absolute paths
If a processor absolutely must use absolute paths (e.g., for a tool that doesn't respect current directory), it should:
1. Store the `project_root` in the processor struct
2. Join paths with `project_root` only at execution time
3. Never store absolute paths in `Product.inputs` or `Product.outputs`

### Why relative paths?
- **Cache portability** — cache keys don't include machine-specific absolute paths
- **Remote cache sharing** — same project checked out to different paths can share cache
- **Simpler code** — no need to strip prefixes for display or storage
