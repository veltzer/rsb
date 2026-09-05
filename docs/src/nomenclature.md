# Nomenclature

This page defines the terminology used throughout RSConstruct's code, configuration, CLI, and documentation.

## Core concepts

| Term | Definition |
|---|---|
| **pname** | Processor name. The type name of a processor as registered by its plugin (e.g., `ruff`, `pip`, `tera`, `creator`). Unique across all plugins. Used in `[processor.PNAME]` config sections and in `processors defconfig PNAME`. |
| **iname** | Instance name. The name of a specific processor instance as declared in `rsconstruct.toml`. For single-instance processors, the iname equals the pname (e.g., `[processor.ruff]` → iname is `ruff`). For multi-instance processors, the iname is the sub-key (e.g., `[processor.creator.venv]` → iname is `creator.venv`). Used in `processors config INAME`. |
| **processor** | A configured instance that discovers products and executes builds. Created from a plugin + TOML config. Immutable after creation. |
| **plugin** | A factory registered at compile time via `inventory::submit!`. Knows how to create processors from TOML config. Has a pname, a processor type, and config metadata. |
| **product** | A single build unit with inputs, outputs, and a processor. The atomic unit of incremental building. |
| **processor type** | One of four categories: `checker`, `generator`, `creator`, `explicit`. Determines how inputs are discovered, how outputs are declared, and how results are cached. See [Processor Types](processor-types.md). |
| **analyzer** | A dependency scanner that runs after product discovery to add extra input edges to existing products (e.g., the `cpp` analyzer adds every `#include`d header as an extra input of a C/C++ product). Analyzers never create products of their own. Declared with `[analyzer.NAME]` sections in `rsconstruct.toml`. Unlike processors, only analyzers explicitly declared in config run — there is no "auto-enable" default. See [Dependency Analyzers](analyzers.md). |
| **analyzer plugin** | A factory registered at compile time via `inventory::submit!` in the analyzer registry. Knows how to construct an analyzer from its `[analyzer.NAME]` TOML section. Each plugin declares its name, description, and whether it is `native` (pure Rust) or external (may invoke subprocesses). |
| **native analyzer** | An analyzer whose default configuration runs entirely in-process (no subprocesses). Example: `icpp` uses a pure-Rust regex scanner for `#include` directives. Some native analyzers become external in non-default configurations (e.g., `icpp` with `pkg_config` set shells out to `pkg-config` for include paths). |
| **external analyzer** | An analyzer that shells out to another program to do its work. Example: `cpp` always runs `gcc -MM` for exact compiler-accurate header scanning. |

## Configuration

| Term | Definition |
|---|---|
| **output_files** | List of individual output files declared in creator/explicit config. Cached as blobs. |
| **output_dirs** | List of output directories declared in creator/explicit config. All files inside are walked and cached as a tree. |
| **src_dirs** | Directories to scan for input files. |
| **src_extensions** | File extensions to match during scanning. |
| **dep_inputs** | Extra files that trigger a rebuild when their content changes. |
| **dep_auto** | Config files added as dep_inputs (e.g., `.eslintrc`). Processor defaults are skipped when absent; entries listed in the config must exist unless `[build] allow_missing_dep_auto` is set. |

## Cache

| Term | Definition |
|---|---|
| **blob** | A file's raw content stored in the object store, addressed by SHA-256 hash. Blobs have no path — the consumer knows where to restore them. |
| **tree** | A serialized list of `(path, mode, blob_checksum)` entries describing a set of output files. Stored in the descriptor store. |
| **marker** | A zero-byte descriptor indicating a checker passed. Its presence is the cached result. |
| **descriptor** | A cache entry (blob reference, tree, or marker) stored in `.rsconstruct/descriptors/`, keyed by the descriptor key. |
| **descriptor key** | A content-addressed hash of `(pname, config_hash, variant, input_checksum)`. Changes when processor config or input content changes. Does NOT include file paths — renaming a file with identical content produces the same key. |
| **input checksum** | Combined SHA-256 hash of all input file contents for a product. |

## Build pipeline

| Term | Definition |
|---|---|
| **discover** | Phase where processors scan the file index and register products in the build graph. |
| **classify** | Phase where each product is classified as skip, restore, or build based on its cache state. |
| **execute** | Phase where products are built in dependency order. |
| **anchor file** | A file whose presence triggers a creator processor to run (e.g., `Cargo.toml` for cargo, `requirements.txt` for pip). |

## CLI conventions

| Command | Name parameter | Meaning |
|---|---|---|
| `processors defconfig PNAME` | pname | Processor type name — shows factory defaults |
| `processors config [INAME]` | iname | Instance name from config — shows resolved config |
| `processors files [INAME]` | iname | Instance name from config — shows discovered files |
| `analyzers defconfig [NAME]` | analyzer name | Analyzer name from the analyzer registry — shows factory defaults |
| `analyzers config [NAME]` | analyzer name | Analyzer name as declared in `[analyzer.NAME]` — shows resolved config |
