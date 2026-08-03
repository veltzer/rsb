# TODO

## StandardConfig refactoring (DONE)

All config structs now embed `StandardConfig` via `#[serde(flatten)]`.

## Cache cleanup

- Remove old DB cache code: `CacheEntry`, `OutputEntry`, `get_entry`, `has_cache_entry`, `get_cached_input_checksum`, `CACHE_TABLE`. These are legacy from the pre-descriptor system. `has_cache_entry` (used in status display to distinguish "stale" vs "new") should use the descriptor system instead. ~80 lines of dead code.

- Split db.redb: the configs table (`CONFIGS_TABLE`) is still in the same DB as the now-unused cache table. Give configs its own file (`configs.redb`), then delete `db.redb` entirely.

## Cache correctness

- Implement `output_depends_on_input_name` flag. Documented as a known limitation in `docs/src/internal/cache.md` but not implemented. Needed for processors that embed the input filename in their output (e.g., a `// Generated from foo.c` header). Without it, renaming such a file would produce a cache hit with stale content.

- Write a test for identical content processed by different processors. Verify two different processors processing the same file get separate cache entries (the processor name is in the descriptor key).

## Code consolidation

- Inline single-use `names` constants. 20+ constants in `processors::names` are used in exactly one place each (their processor's `new()` call). Inline them as string literals.

- Clean `processor_configs.rs`. Still 2,100+ lines. Check for:
  - ClangTidyConfig is nearly identical to StandardConfig — could it become a type alias?
  - Unused `default_*` helper functions left over from cppcheck removal.
  - Other config structs that are structurally identical to StandardConfig.

## Documentation

- Add `docs/src/processors/creator.md` — per-processor documentation for the Creator processor, like the other processor docs.

## Housekeeping

- Remove the `tar` lockfile entries. The crate was added and removed, but `Cargo.lock` may still reference it.

- Commit everything. There is a large amount of uncommitted work spanning:
  - HasScanConfig trait elimination
  - SimpleGenerator (14 generators collapsed to data-driven)
  - Creator processor (new processor type with multi-dir caching)
  - Cache redesign (descriptor-based, content-addressed keys, no DB for cache data)
  - Checksum cache centralization (moved mtime logic to `checksum.rs` with own DB)
  - MassGenerator → Creator type rename
  - `ProcessorType` enum with strum iteration
  - `processors types` CLI command
  - `--no-mtime-cache` CLI flag
  - Mandatory `supports_batch` on all processors
  - Checker consolidation (5 checkers → SimpleChecker entries)
  - Removed unused `dirs` crate
  - New documentation: cache.md, checksum-cache.md, processor-types.md
