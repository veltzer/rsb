# Creator Processor

## Why "creator"?

Checkers validate files and produce nothing. Generators transform one input
into one output. Neither shape fits a tool that runs once and produces an
opaque mass of files — a virtualenv, a `node_modules` tree, a rendered site.
The Creator type covers that case: run a command, then cache whatever the user
declares the command produced.

Names considered:

- **creator** — chosen. It *creates* a body of output rather than transforming
  a file into a file.
- **mass_generator** — taken by a different, more demanding contract (the tool
  must enumerate its outputs in advance). See
  [MassGenerator](mass_generator.md).
- **installer** — too narrow; package installation is one use, not the type.
- **builder** — collides with rsconstruct's own `Builder` internals.

## Purpose

Runs a command and caches a declared set of output directories and files.
Unlike a generator (1 input → 1 output) or a checker (validate only), a Creator
runs a command that may produce any combination of files and directories, and
rsconstruct never tries to figure out what those are — the config says so.

Typical uses:

- `pip install -r requirements.txt` producing `.venv/`
- `npm install` producing `node_modules/`
- `mdbook build` producing `book/`
- Any tool whose output tree is opaque but whose *trigger* is a single
  well-known file

The dedicated creators (`cargo`, `cc`, `gem`, `jekyll`, `mdbook`, `npm`,
`pip`, `sphinx`) are purpose-built versions of this shape with their own
discovery rules. The generic `creator` processor is what you configure when
your tool is not one of those.

## How it works

### Anchor files

Creator discovery is **anchor-based**. The scan configuration
(`src_extensions`, `src_files`, `src_dirs`, …) selects a set of files, and each
matched file becomes an *anchor* — one product per anchor. The anchor is the
product's primary input, and the command runs in the anchor's **parent
directory**.

This is why a Creator is usually pointed at a manifest file:
`requirements.txt`, `package.json`, `book.toml`. One manifest → one anchor →
one product, with the command running beside the manifest.

### Output resolution is relative to the anchor

`output_dirs` and `output_files` entries are resolved **relative to the anchor's
directory**, not the project root. An anchor at `subproject/requirements.txt`
with `output_dirs = [".venv"]` produces and caches `subproject/.venv`. An anchor
at the project root resolves them at the project root.

This means one Creator instance handles a monorepo of sibling subprojects
without per-directory configuration.

### Execution

The command runs with `args`, in the anchor's directory. A non-zero exit is a
build failure and the tool's output is reported. There is no
`--inputs`/`--outputs` convention — unlike the
[Explicit processor](explicit.md), a Creator's command receives only what
`args` specifies.

### Caching

If `output_dirs` is non-empty, the product is registered with those directories
as cached units — the entire contents of each directory are stored and restored
as a whole. If `output_dirs` is empty, the product caches only the declared
`output_files`.

Because the output tree is opaque, the cache unit is the whole directory: on a
cache hit the directory is restored wholesale, and there is no per-file
granularity within it. Contrast this with
[MassGenerator](mass_generator.md), where each predicted file is its own
product and its own cache entry.

Creator products are not batched, and the type is capped at one concurrent job
(`max_jobs_cap = 1`) — package managers and site builders generally do not
tolerate concurrent invocations against the same tree.

## Configuration

```toml
[processor.creator.venv]
command        = "pip"
args           = ["install", "-r", "requirements.txt"]
src_files      = ["requirements.txt"]
output_dirs    = [".venv"]
```

A Creator whose tool emits a couple of known files rather than a tree:

```toml
[processor.creator.protoc]
command      = "make"
args         = ["generate"]
src_files    = ["Makefile"]
output_files = ["gen/api.pb.go", "gen/api_grpc.pb.go"]
```

### Fields

| Key | Type | Required | Checksum | Description |
|---|---|---|---|---|
| `command` | string | no | yes | Binary to execute. A Creator with no command is a config error at execute time. |
| `args` | array of strings | no | yes | Arguments passed to the command |
| `output_dirs` | array of strings | no | yes | Directories to cache after the command runs, relative to the anchor |
| `output_files` | array of strings | no | yes | Individual files to cache after the command runs, relative to the anchor |
| `dep_inputs` | array of strings | no | no | Extra files that invalidate the product when changed |
| `dep_auto` | array of strings | no | no | Automatic dependency analyzers to run |
| `max_jobs` | int | no | no | Per-instance job cap (the type is capped at 1 regardless) |
| `enabled` | bool | no | no | Set false to disable the instance |
| `src_*` | array of strings | no | no | Standard scan fields selecting the anchor files |

`formats` and `output_dir` are omitted from this processor — they describe the
1:1 generator shape, which does not apply.

Run `rsconstruct processors defconfig creator` for the authoritative field list
with current defaults.

## Cross-processor dependencies

Declared outputs participate in the graph like any others. A Creator that
declares `output_files` gives downstream processors precise edges to depend on.

A Creator that declares only `output_dirs` is **opaque**: the individual files
inside are not products, so a downstream processor cannot form an edge to any
one of them. To lint or post-process the contents of a Creator's output
directory, name that directory in the downstream processor's `src_dirs` — which
force-walks it despite the output-root exclusion (see
[File indexing](../internal/architecture.md#file-indexing)) — and add an
explicit ordering relationship if the timing matters. See
[Shared Output Directory](../internal/shared-output-directory.md) for the
mechanism that keeps two processors writing into one directory from fighting.

## Comparison with other processor types

| | Checker | Generator | Creator | Explicit |
|---|---|---|---|---|
| Products | one per input file | one per input file | one per anchor | one total |
| Outputs | none (pass/fail) | one per input | declared dirs/files | explicitly listed |
| Discovery | scan config | scan config | scan config (anchors) | declared inputs/globs |
| Cache unit | marker | per file | whole directory | per file |
| Cwd for command | project root | project root | anchor's directory | project root |
| Use case | lint/validate | transform 1:1 | install/build a tree | aggregate many → few |

## Clean behavior

This is a Creator processor — `rsconstruct clean outputs` removes each declared
`output_dirs` entry recursively. Declared `output_files` entries are **not**
removed by clean; only directories are. After all per-product cleans complete,
the orchestrator removes any parent directories that are now empty. Pass
`--no-empty-dirs` to keep them. See
[Clean behavior](../processors.md#clean-behavior) and
[`rsconstruct clean`](../commands.md#rsconstruct-clean).

## See also

- [MassGenerator](mass_generator.md) — the transparent counterpart, for tools
  that can enumerate their outputs in advance
- [Explicit](explicit.md) — for declared-inputs-to-declared-outputs build steps
- [Shared Output Directory](../internal/shared-output-directory.md)
- [Cache System](../internal/cache.md)
