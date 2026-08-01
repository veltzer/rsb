# Environment Variables

## The problem

Build tools that inherit the user's environment variables produce non-deterministic builds. Consider a C compiler invoked by a build tool:

- If the user has `CFLAGS=-O2` in their shell, the build produces optimized output.
- If they unset it, the build produces debug output.
- Two developers on the same project get different results from the same source files.

This breaks caching (the cache key doesn't account for env vars), breaks reproducibility (builds differ across machines), and makes debugging harder (a build failure may depend on an env var the developer forgot they set).

Common examples of environment variables that silently affect build output:

| Variable | Effect |
|---|---|
| `CC`, `CXX` | Changes which compiler is used |
| `CFLAGS`, `CXXFLAGS`, `LDFLAGS` | Changes compiler/linker flags |
| `PATH` | Changes which tool versions are found |
| `PYTHONPATH` | Changes Python module resolution |
| `LANG`, `LC_ALL` | Changes locale-dependent output (sorting, error messages) |
| `HOME` | Changes where config files are read from |

## RSConstruct's approach

RSConstruct does **not** use environment variables from the user's environment to control build behavior. All configuration comes from explicit, versioned sources:

1. **`rsconstruct.toml`** — all processor configuration (compiler flags, linter args, scan dirs, etc.)
2. **Source file directives** — per-file flags embedded in comments (e.g., `// EXTRA_COMPILE_FLAGS_BEFORE=-pthread`)
3. **Tool lock file** — `.tools.versions` locks tool versions so changes are detected

This means:

- The same source tree always produces the same build, regardless of the user's shell environment.
- Cache keys are computed from file contents and config values, not ambient env vars.
- Remote cache sharing works because two machines with different environments still produce identical cache keys for identical inputs.

## Rules for processor authors

When implementing a processor (built-in or Lua plugin):

1. **Never read `std::env::var()`** to determine build behavior. If a value is configurable, add it to the processor's config struct in `rsconstruct.toml`.

2. **Never call `cmd.env()`** to pass environment variables to external tools, unless the variable is derived from explicit config (not from `std::env`). The user's environment is inherited by default — the goal is to avoid *adding* env-based configuration on top.

3. **Tool paths come from `PATH`** — RSConstruct does inherit the user's `PATH` to find tools like `gcc`, `ruff`, etc. This is acceptable because each tool's identity is part of the cache key of everything it produces: resolving a different binary, or the same binary with different content, invalidates that processor's cached results. See [`hash_tool_versions`](configuration.md#build). `rsconstruct tools lock` additionally pins exact versions and makes drift a hard error.

4. **Config values, not env vars** — if a tool needs a flag that varies per project, put it in `rsconstruct.toml` under the processor's config section. Config values are hashed into cache keys automatically.

## What RSConstruct does inherit

RSConstruct inherits the full parent environment for subprocess execution. This is unavoidable — tools need `PATH` to be found, `HOME` to read their own config files, etc. The key design decision is that RSConstruct itself never *reads* env vars to make build decisions, and processors never *add* env vars derived from the user's environment.

The exceptions are:

- `NO_COLOR` — RSConstruct respects this standard env var to disable colored output, which is a display concern and does not affect build output.
- `RSCONSTRUCT_THREADS` — Sets the number of parallel jobs (equivalent to `-j`). Priority: CLI `-j` flag > `RSCONSTRUCT_THREADS` env var > `[build] parallel` config. This is a performance tuning concern and does not affect build correctness or output.
