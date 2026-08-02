# RSConstruct - Rust Build Tool

A fast, incremental build tool written in Rust with tera support, Python linting, and parallel execution.

Detailed documentation is in `docs/src/`. Key references:
- Commands: `docs/src/commands.md`
- Configuration: `docs/src/configuration.md`
- Architecture (subprocess execution, path handling, caching): `docs/src/internal/architecture.md`
- Processor contract: `docs/src/internal/processor-contract.md`
- Coding standards: `docs/src/internal/coding-standards.md`
- Per-processor docs: `docs/src/processors/`

## Philosophy

- **Simplicity first** — keep the code simple whenever possible. Avoid clever solutions that are hard to understand or maintain. When in doubt, choose the straightforward approach.
- **Convention over configuration** — simple naming conventions, explicit config loading, incremental builds by default.
- **No macros** — the codebase has zero `macro_rules!` and must stay that way. Use regular functions, generics, traits, and structs to eliminate duplication. Do not add new macros. (The former `ctx!` exception is gone: `errors::ctx()` is `#[track_caller]`, so it reports the caller's file:line without a macro. Note `tests/` still uses `test_checker!`; test-only macros are out of scope for this rule.)
- **Unix-only; OS calls live in `src/platform.rs`** — RSConstruct targets Linux and macOS only (see `docs/src/internal/rejected-problems.md`). All OS-specific code (file permissions, signal handling) lives in `src/platform.rs`, which the rest of the codebase calls through named wrappers. Do not add `#[cfg(...)]` blocks anywhere, including `platform.rs`: there is no second platform to switch on, and a `#[cfg(not(unix))]` branch is dead code that cannot be compiled or tested. Unix assumptions elsewhere (`flock`, `/dev/null`, `$HOME`, apt) are deliberate, not bugs.
- **Strict by default** — never silently skip errors or ignore failures. Non-strict systems hide problems and are a disaster. If a tool is missing, fail. If a test fails, fix it before moving on.
- **All tests must pass** — always run `cargo test` with no filters or skips. Do not move forward with any failing test. If a test fails, fix it immediately — the failure is real.
- **No scripts to modify code** — never use Python scripts, sed, awk, or any external tool to modify Rust source code. All code changes must be made manually through the editor. Automated bulk changes produce inconsistent results and hide mistakes.
- **Always add context to errors** — every `?` on an IO operation (`fs::read`, `fs::write`, `Command::spawn`, `fs::create_dir_all`, etc.) must have `.with_context(|| format!("..."))` that says what you were trying to do and which file/command was involved. A bare `?` on an IO operation is a bug — it produces useless error messages like "No such file or directory" with no indication of what went wrong. Use `anyhow::Context` everywhere.
- **Never create dummy instances** — never instantiate a processor (or any object) just to inspect its metadata. Metadata (config fields, defaults, descriptions) must be available without creating an instance. If you need config info, get it from the plugin interface, not from a throwaway instance.
- **CLI subcommands are always alphabetical** — every `#[derive(Subcommand)] enum` in `src/cli.rs` (top-level `Commands` and every `*Action` enum) must list its variants in alphabetical order by display name (clap's kebab-case conversion of the variant — e.g. `EnableDetected` → `enable-detected`). Clap renders subcommands in declaration order, so this list IS the help output. When adding a new variant, insert it at its alphabetical position. No exceptions.
- **All tunable behavioral knobs are config fields** — any value that affects runtime behavior and could reasonably vary per project must be a `pub` field on a config struct (per-processor `*Config` in `src/config/processor_configs.rs`, or `BuildConfig` in `src/config/mod.rs` for cross-cutting build-wide settings), with a `#[serde(default = "...")]` defaulting to the historical hardcoded value. This includes timeouts, retry counts, max-attempts, batch sizes, poll intervals, size/length limits, dictionary paths, output caps. It does NOT include regex patterns, internal algorithm constants, enum discriminators, or constants dictated by an external file format. The field must also be added to `known_fields()`, `field_descriptions()`, and — if it changes the produced output bytes — `checksum_fields()`. A hardcoded `const FOO_TIMEOUT: Duration = ...` (or similar magic literal in code) inside `src/processors/` is a bug. Use `rsconstruct processors defconfig <name>` to verify the field is exposed.
