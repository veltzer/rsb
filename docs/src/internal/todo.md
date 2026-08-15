# TODO

## Cache correctness

- Implement `output_depends_on_input_name` flag. Documented as a known
  limitation in `cache.md` but not implemented. Needed for processors that
  embed the input filename in their output (e.g., a `// Generated from foo.c`
  header). Without it, renaming such a file produces a cache hit with stale
  content. The complementary by-design behavior is covered by
  `cache_survives_input_rename` (`tests/tests_mod/cache.rs`).

## Housekeeping

- Split `db.redb`. `CONFIGS_TABLE` is now the only table in it
  (`src/object_store/mod.rs`), so renaming the file to `configs.redb` is a
  pure cosmetic rename with no correctness argument behind it. Low priority.
