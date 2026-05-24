# Future Features

A backlog of potential features for rsconstruct, grouped by theme. Each entry
notes how it relates to the existing codebase so the design fits the current
architecture.

## Remote / distributed builds

### Finish the remote cache pull path
`object_store/operations.rs` already has `try_fetch_object_from_remote` and
`try_fetch_descriptor_from_remote` scaffolded but marked `#[allow(dead_code)]`
("not yet called from any read path"). Wiring these into
`needs_rebuild_descriptor` / `can_restore_descriptor` would give true shared
caching — CI populates the cache, developers restore from it. Highest-leverage
item: the scaffolding already exists. The README currently advertises "remote
caching — share build artifacts across machines", but only the push path works;
this feature is what makes that claim true.

### Remote / distributed execution
The graph already computes parallel levels and per-processor concurrency.
Extending the executor to dispatch products to remote workers (gRPC/SSH) would
turn rsconstruct into a Bazel-lite for polyglot projects.

## Build intelligence

### Build profiling & critical-path analysis
`--timings` already records per-product durations and `start_offset`. A
`rsconstruct analyze timings` command could surface the critical path through
the graph, the slowest processors, and parallelism efficiency (wall time vs.
summed CPU time) — telling users where to add `max_jobs` or split work.

### Cache-hit telemetry / `why-rebuild`
`--explain` shows per-product action. A persistent log of *why* each product
rebuilt (input changed / config changed / tool version changed / dep changed)
would help diagnose "why is my incremental build not incremental." The
`descriptor_key` already mixes all those inputs — the diff just needs to be
reported.

### `rsconstruct query` — graph queries
The graph supports DOT/Mermaid/JSON export. A query language ("what depends on
`foo.h`", "what does `make-docs` produce", "show the path from A to B") would
help large projects.

## Developer experience

### LSP / editor integration
A language server for `rsconstruct.toml` giving completion of processor names,
config field names (`known_fields()` / `field_descriptions()` exist per
processor), and inline validation. The metadata is all there.

### `rsconstruct doctor --fix`
`doctor` already exists; auto-installing missing tools via the
`DependenciesConfig` (pip/npm/gem) and offering to add `[processor.*]` stanzas
for auto-detected file types would smooth onboarding.

### Build watch with TUI
The watcher currently prints lines. A live TUI showing the graph, in-flight
products, cache hit rate, and per-processor progress would be a strong UX
upgrade.

## Correctness / reproducibility

### Hermetic / sandboxed execution
Run each processor in a sandbox that only exposes its declared inputs, catching
under-declared dependencies (the classic incremental-build bug). The graph
already knows every product's exact input set.

### Reproducibility verification mode
The `BuildPolicy` trait doc explicitly mentions "deterministic-verification
modes" as a future implementation. A policy that rebuilds and byte-compares
against the cached blob would catch non-deterministic processors.

### Content-addressed remote artifact sharing / `rsconstruct fetch`
Since blobs are already content-addressed and path-free, a command to pull a
specific built artifact by target name from the cache (without a full build) is
natural.

## Ecosystem

### Richer plugin SDK
Lua plugins exist (`src/processors/lua/`). A documented, versioned plugin API
plus a plugin registry/marketplace would let the community add processors
without forking.

### CI provider integrations
First-class GitHub Actions / GitLab CI templates that set up the remote cache,
run `tools verify` against the lock file, and emit the JSON build events
(`json_output.rs`) as CI annotations.

### `rsconstruct migrate`
Importers from `Makefile`, `CMakeLists.txt`, or `package.json` scripts that
generate a starter `rsconstruct.toml`.
</content>
</invoke>
