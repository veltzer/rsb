# Architecture Review Findings

Full-codebase architecture audit (2026-07-18). Distinct from `CODE_REVIEW_FINDINGS.md` (line-level bugs, all fixed): these are structural/design problems. Checkboxes track future remediation work.

Recurring failure pattern across the codebase: **knowledge duplicated across hand-synchronized places with nothing enforcing agreement**, plus **advertised contracts the code doesn't implement**. The line-level code is careful; the seams between subsystems are where the debt lives.

## High severity

- [ ] 1. **Schema has no single source of truth — drift has already happened.** One config field's schema lives in 5–8 hand-synced places (struct field, serde default fn, `Default` impl, `known_fields()`, `field_descriptions()`, `checksum_fields()`, `expected_field_type` arm in `src/config/mod.rs:1205`, docs page). Adding a processor also touches central tables (`scan_defaults_for` ~92 arms, `processor_defaults_for` ~71 arms, `src/config/mod.rs:511-692`). Proven drift: dead `expected_field_type` arms for renamed fields (`marp_bin`, `mmdc_bin`, `drawio_bin`, `chromium_bin`, `libreoffice_bin`, `markdown_bin`, `checker`, `linter`); marp `args` default encoded twice with different values (`MarpConfig::default()` vs `processor_defaults_for("marp")`); marp `timeout_secs`/`max_attempts` missing from type validation. Worst consequence: a forgotten `checksum_fields` entry on an output-affecting field = **silently stale build outputs**. Fix direction (no macros): one static `FieldSpec { name, ty, description, in_checksum, required }` table per config, carried on the `ProcessorPlugin` inventory entry, from which known-fields/types/descriptions/defaults all derive; one test iterating `all_plugins()` asserting coverage. Invert the checksum default (in unless opted out with a reason) so mistakes cause spurious rebuilds instead of stale outputs.

- [ ] 2. **Three advertised contracts are not real.**
  - Fail-fast batching is dead code: both `None` arms of the chunk-size logic are unreachable (`src/executor/execution.rs:381-386,766` vs `src/builder/build.rs:272`); default fail-fast sends the whole level in one chunk and `execute_checker_batch` smears one exit code over every product — architecture.md promises chunk-size-1 incremental recovery that doesn't exist.
  - Phase hooks (`src/phases.rs`): 5 of 6 phases never fire; `run_phase` is called exactly once (PostConfig, `src/config/mod.rs`); hooks receive only `&mut Config` so pipeline phases couldn't observe the graph anyway. The pipeline is enumerated 4 uncoordinated times (`phases::Phase`, `cli::BuildPhase`, `GraphSnapshot`, string-keyed `phase_timings`).
  - Docs describe removed/nonexistent systems: architecture.md's interrupt section (global flag + 50 ms poll — code uses a tokio watch channel), a four-method `ProductDiscovery` trait (doesn't exist), cache.md's "no separate database / descriptors live in objects/" (there is `descriptors/` + `db.redb`), `output_depends_on_input_name` (zero hits in src/), and `processors/mod.rs:26` references a `src/registry.rs` that doesn't exist.

- [ ] 3. **Cache-key composition is decentralized, with correctness holes behind it.** Key material assembled in ≥5 uncoordinated places (`Product::descriptor_key` `src/graph.rs:98`, `output_config_hash` allowlist, `extend_config_hash`, `apply_tool_version_hashes` — which uses a different combination style — and per-processor `checksum_fields`). Concrete holes:
  - Creator `output_dirs`/`output_files` are excluded from the config hash with a comment claiming outputs are "part of the product's identity" — true only of the legacy key. **Changing a creator's output_dirs = permanent silent SKIP.**
  - `cache = false` (`StandardConfig.cache`) is documented, validated, described — and read by no code. A documented no-op.
  - Tool-version keying is opt-in: without `tools lock`, upgrading a tool leaves all cached PASSes valid.
  - Legacy `CacheEntry`/`CACHE_TABLE` is never written but still consulted by status (`builder/build.rs` stale/new split) — every non-current product reports "new".
  Fix direction: a single `CacheKey` builder owning every component; delete legacy types; hash installed tool versions by default.

- [ ] 4. **Testing is architecturally unenforced.** CI workflows contain no `cargo test` at all. ~150 of ~436 integration tests silently self-disable when a tool is missing (82 `tool_available()` early-return sites + the `test_checker!` macro's skip guards) — a skip is indistinguishable from a pass, in a project whose creed is "never silently skip." Near-zero unit tests in core modules: all of `src/executor/`, all of `src/builder/`, 8 of 9 `src/object_store/` files, `src/checksum.rs`, `src/watcher.rs`; everything is exercised through 569 spawns of the real binary. Fix direction: CI job with the full tool matrix + a `RSCONSTRUCT_TEST_REQUIRE_TOOLS=1` mode turning skips into failures; unit-test the pure cores (executor policy/retry, checksum, object-store validity/restore, run_checker chunking).

- [ ] 5. **No output layer — the `--json` contract is unenforceable.** 437 `println!/eprintln!` sites across 32 modules; JSON gating checked in only ~17 files, `quiet()` at 8 sites. Verbose/colored lines leak into `--json` stdout today (`src/executor/execution.rs:408,523,735`; unconditional `println!` in `flush_words`), and the test harness tolerates it (`BuildResult::parse` discards non-JSON lines). `runtime_flags` itself is fine (write-once OnceLock); the missing piece is a single emit-sink. Fix direction: one `output` module (`emit_human`/`emit_event` consulting quiet/json/color), migrate call sites, and a test asserting every stdout line under `--json -v` parses as JSON.

## Medium severity

- [ ] 6. **God modules and inverted layering.**
  - `src/processors/mod.rs` (1,897 lines) = subprocess infra + tool-install engine + `ToolInfo` table + build stats + the actual processor contract; graph/analyzers/executor import "processors" for non-processor reasons. Extract `exec`, `tools`, `stats` modules.
  - `src/graph.rs` (1,330 lines) mixes the core data model with six renderers and imports `crate::cli` types (`DisplayOptions` etc.) and spawns Graphviz; `builder/tools.rs` duplicates a parallel renderer set. Move renderers out; move display types to a neutral module.
  - Module cycle: `processors → config → registries → processors` — config is not a leaf layer.
  - `src/main.rs` (~958 lines): ~620-line `run()` with inline command bodies (Cache handling, init_project scaffolding, formatting per match arm).
  - Tool installation split across two layers (`processors/mod.rs` engine + `builder/tools.rs` orchestration, whose `run_tools_command` is ~620 lines, with a duplicated `tool_runtime` helper).
  - Visibility is convention-only (~80 `pub(crate)/pub(super)` in 35k lines); none of the layer boundaries are compiler-enforced.

- [ ] 7. **`Builder` is a god object; the pipeline is not a first-class thing.** One struct, impl blocks in 10 files, ~35 public entry points; `Builder::build` (~240 lines) mixes CLI semantics (alias expansion), config mutation, tool preflight, orchestration, presentation, and exit-code mapping. Nothing between "whole build" and "one product" is testable/reusable; a daemon/LSP mode (anticipated by `BuildContext`'s docs) would have to re-enter through the CLI-shaped surface. Fix direction: extract a `BuildPipeline` type (discover→analyze→resolve→classify→execute) with a reporter interface; `Builder` becomes the thin command dispatcher.

- [ ] 8. **Processor trait + capability model accretion.**
  - 15 methods, 2 required; default `discover()` panics at runtime (`.expect(...)`) — a required method disguised as a default (38 impls override it); `scan_config()`/`standard_config()` are duplicate accessors; `discover_for_clean`/`tool_version_commands`/`config_has_fix` each have exactly one overrider.
  - Capability drift is live: `fix list` filters on static `can_fix` alone while `fix` also honors `config_has_fix` — a configured `script.fix_command` is fixable but invisible in the list (`builder/fix.rs:117` vs `:21-22`); `black` declares `can_fix: true` with no fix params (works by accident; `supports_fix_batch()` wrongly false, silently disabling batch fixing); `is_native` declared twice per SimpleGenerator.
  - `SimpleChecker`/`SimpleGenerator` are hardwired to `StandardConfig`: one extra config field forces a ~100-line hand-rolled processor (pandoc, aspell). Fix: make them generic over `C: Serialize + KnownFields + AsRef<StandardConfig>` — plain generics, no macros.
  - Discovery contract inconsistently honored: tags and pdfunite/ipdfunite walk the filesystem directly (evading ignore rules and virtual files, re-running IO every fixed-point pass) despite architecture.md's "processors never walk the filesystem".

- [ ] 9. **Executor scheduling wastes parallelism structurally.** Level-barrier model: the longest product in a level stalls the entire next level even for products whose real dependencies finished; one thread per batch group; static contiguous chunk partitioning with no work stealing; a thread blocked on a `max_jobs` permit can't take other work. The adjacency lists/failure propagation/semaphores needed for a ready-queue scheduler already exist. This is the one *expensive* fix — gate it on a `--trace` profile of a real project. Related: "needs rebuild" is computed three times (classify, per-level, post-execution) with intentional divergence held together by comments; reify estimate-vs-authoritative checksums as distinct types.

- [ ] 10. **Graph identity is maintained by convention.** `Product.id == index` across three parallel Vecs, upheld manually; `retain_products` deliberately leaves stale indexes (doc-comment guard only); `filter_by_targets` rebuilds the world, duplicating `add_product`'s registration logic (two sites that must evolve in lockstep), and takes **no transitive closure** — a `-t` build can keep a consumer while dropping its producer. Checker identity `(processor, primary_input, variant)` is a second, weaker scheme: same-primary-input checkers collide by design, and a non-superset re-declaration is silently ignored where the generator path hard-errors. Fix direction: one canonical rebuild path; explicit product identity keys; close over upstream producers in target filtering.

- [ ] 11. **Config pipeline fragility beyond the schema tables.**
  - Textual substitution can't see partial references: `src_dirs = ["${base}/src"]` matches neither substitution nor the undefined-var scan — flows through silently as a literal, contradicting the documented "undefined references produce an error". No escape mechanism for literal `${...}`. `extract_var_names` naively splits on `=` (multi-line arrays with `a=b` items record junk names). Fix: substitute on the parsed value tree (toml_edit), which also eliminates the provenance line-number coupling.
  - `is_multi_instance` heuristic: instance names colliding with any known field silently reinterpret the section; adding a field in a future release can retroactively change how an existing user's config parses; Lua plugins can never be multi-instance, no diagnostic. Make the shape explicit, hard-error on ambiguity.
  - `--iset`/`--pset` are a second, weaker validation path: skips `must_fields`, the src_dirs rule, and the `max_jobs > 0` guard (`--iset x.max_jobs=0` bypasses the deadlock protection); override ordering makes the `CliOverride` provenance arm in `apply_output_dir_defaults` unreachable. Route overrides through `validate_single_processor`.
  - Provenance spans depend on a cross-module invariant (blanked vars lines + no-newline inline values) stated only in comments; span-walk multi-instance heuristic duplicated with different logic than the parser's.
  - `Config::load` parses the same bytes ~5 times across two parser crates.

- [ ] 12. **Object store: two engineering standards side by side.**
  - Blobs: atomic, format-tagged, read-only, verified (exemplary). Descriptors: plain `fs::write` over a read-only file with an acknowledged chmod-retry race; a torn descriptor permanently breaks `cache trim`. Apply the blob discipline (temp+rename) to descriptors.
  - Marker/Blob/Tree: ~20 match arms across 7 functions with real drift — `explain_descriptor` existence-checks where `needs_rebuild_descriptor` content-verifies (`--explain` can disagree with the build); Tree arms ignore `output_paths` entirely. Normalize to one `Vec<TreeEntry>` model (Marker = empty, Blob = single pathless entry).
  - Five persistence mechanisms (db.redb, mtime.redb, deps.redb, webcache.redb, JSON descriptors + blobs) with three concurrency models; the `db.redb` open-lock makes two concurrent rsconstruct processes impossible anyway, so the multi-process blob safety is unreachable. Decide single-process vs multi-process and align.
  - Remote cache is half a feature: blob push works (one exists-check + one upload subprocess per object, serial), descriptor push/fetch and object fetch are `#[allow(dead_code)]` — a populated bucket can never satisfy a restore. Land pull or error on the config.
  - Output verification bypasses the mtime cache: `needs_rebuild_descriptor` full-hashes every cached output byte via a function documented as "not hot-path", twice per no-op build (classify + per-level), third time on restore — catastrophic for cached trees like `.venv`. Route through `checksum_fast` or store `(mtime, size)` in tree entries.

- [ ] 13. **Subprocess execution and exit codes.**
  - The central `run_command` path is excellent (tokio-select interrupt, kill_on_drop, timeout, log_command, declared-tools debug check) but ~26 spawn sites in 9 files bypass it — including inside the processor layer (aspell's stdin-feeding spawn, `dot_to_svg`, linux_module's `uname`, git calls in builder/analyzers) — losing interrupt handling and the declared-tools check where drift is most likely. Add a stdin variant to the central path; route everything through it.
  - `classify_error`'s fallback substring-matches the rendered error chain, which embeds subprocess stderr: tool output containing "unknown field" → exit 2; "interrupted" → 130. The typed `RsconstructError` path already covers the main producers and survives context wrapping — delete the string-matching fallback (keep the io::Error chain check), type the stragglers, default to BuildError.
  - Interrupt state is triplicated: standalone `Arc<AtomicBool>` threaded through signatures + `BuildContext.interrupted` + the tokio watch channel; the executor ORs the first two. Consolidate into BuildContext (its own docs say that was the plan).

## Low severity

- [ ] 14. **Cross-platform support is aspirational.** cfg discipline is perfect (zero `#[cfg]` outside platform.rs), but unix assumptions live above it: hard `flock` dependency (libreoffice), `/dev/null` in curl args (remote_cache), `$HOME` tilde expansion, apt-centric install methods, Linux-only release CI. The Windows branches in platform.rs have plausibly never been compiled. Either add a `cargo check --target x86_64-pc-windows-msvc` CI job or declare unix-only and delete the branches.
- [ ] 15. **Watch mode discards `event.paths`** — full Builder + FileIndex + discovery + classify on every debounced event; latency scales with project size, not change size. Acceptable today; blocks future incrementality (same root as finding 7).
- [ ] 16. **Unbounded growth:** webcache stores full HTTP bodies keyed by URL forever (no TTL/eviction, reopens its DB per fetch, duplicates the object store's job); mtime.redb accumulates entries for deleted files. Fold webcache into the CAS; prune mtime entries during `cache trim`.
- [ ] 17. **Vestigial code:** `scan_root_valid` always returns true, called ~20 times; `ctx!` macro still marked "will be removed once migrated"; `main.rs` hand-resolves scan defaults for `terms` bypassing the defaults framework; fixed-point discovery loop hits its 10-pass cap silently without erroring or injecting the final pass's virtual files.

## Concrete bugs surfaced by this review (fixable independently of refactors)

- [ ] B1. Creator `output_dirs`/`output_files` change never invalidates cache → permanent stale SKIP (finding 3)
- [ ] B2. `cache = false` config field is a no-op (finding 3)
- [ ] B3. `--iset <x>.max_jobs=0` bypasses the load-time deadlock guard; overrides skip semantic validation generally (finding 11)
- [ ] B4. `filter_by_targets` takes no transitive closure — `-t` can drop a kept consumer's producer (finding 10)
- [ ] B5. `fix list` and `fix` disagree on what's fixable (`can_fix` vs `can_fix || config_has_fix`) (finding 8)
- [ ] B6. `black` declares `can_fix: true` with no fix params — violates the documented invariant; batch fixing silently disabled for it (finding 8)
- [ ] B7. Status stale/new distinction reads a table nothing writes — always reports "new" (finding 3)
- [ ] B8. `--json -v` leaks non-JSON lines onto stdout (finding 5)
- [ ] B9. `"${var}/suffix"` partial variable references pass through silently as literals (finding 11)
- [ ] B10. Fixed-point discovery non-convergence at the 10-pass cap is silent (finding 17)

## Suggested order of attack

1. **Cheap + high value:** the `FieldSpec` consolidation (collapses findings 1, parts of 3 and 8); delete the fake contracts (dead batching arms, unfired phases, string-match exit fallback, legacy cache table) and re-sync the three stale docs; CI test job + strict-skip mode (finding 4).
2. **The concrete bugs B1–B10** — each is small and independent.
3. **Structural:** output sink (5), descriptor atomicity + Tree normalization (12), Builder/pipeline split (7), module extractions (6).
4. **Only with profiling evidence:** the ready-queue scheduler (9).
