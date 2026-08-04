# Checksum Cache

RSConstruct uses a centralized checksum system (`src/checksum.rs`) for all file hashing. It has two layers of caching to avoid redundant I/O and computation.

## Architecture

All file checksum operations go through a single entry point: `checksum::file_checksum(path)`. This function never computes the same hash twice.

### Layer 1: In-memory cache (per build run)

A global `HashMap<PathBuf, String>` stores checksums computed during the current build. When a file is checksummed for the first time, the result is cached. Any subsequent request for the same file returns the cached value without reading the file again.

This handles the common case where the same file appears as an input to multiple products (e.g., a shared header file), or when the checksum is needed both for classification (skip/restore/build) and for cache storage.

The in-memory cache lives for the duration of the process and is not persisted.

### Layer 2: Mtime database (across builds)

A persistent redb database at `.rsconstruct/mtime.redb` maps file paths to `(mtime, checksum)` pairs. Before reading a file to compute its checksum, the system checks:

1. Has this file been checksummed in a previous build?
2. Has the file's modification time changed since then?

If the mtime matches, the cached checksum is returned without reading the file. This avoids I/O for files that haven't been modified between builds — the common case in incremental builds where most files are unchanged.

When the mtime differs (file was modified), the file is read, the new checksum is computed, and both the in-memory cache and the mtime database are updated.

Dirty mtime entries are flushed to the database in a single batch transaction at the end of each checksum computation pass, minimizing database writes.

### Why two layers

| Layer | Scope | Avoids | Cost |
|---|---|---|---|
| In-memory cache | Single build run | Re-reading + re-hashing the same file | HashMap lookup |
| Mtime database | Across builds | Reading unchanged files from disk | `stat()` + DB lookup |

For the first build, every file must be read and hashed. The mtime database is populated as a side effect. On subsequent builds, most files are unchanged — the mtime check skips reading them entirely, and the in-memory cache prevents redundant lookups within the run.

## Configuration

The persistent mtime database can be disabled via `rsconstruct.toml`:

```toml
[cache]
mtime_check = false
```

Or via the command-line flag:

```bash
rsconstruct build --no-mtime-cache
```

When disabled, every file is read and hashed on every build. The in-memory cache still prevents redundant reads within a single run, but there is no cross-build benefit.

**When to disable:** In CI/CD environments with a fresh checkout, the mtime database has nothing cached from previous builds and just adds write overhead. The in-memory cache is sufficient. Use `--no-mtime-cache` (or `mtime_check = false` in config) to skip the database entirely.

The `rsconstruct status` command also disables mtime checking internally to ensure accurate classification.

## Database location

The mtime database is stored at `.rsconstruct/mtime.redb`, separate from the build cache (`objects/` and `descriptors/`) and the config tracking database. This separation means:

- `rsconstruct cache clear` removes the entire `.rsconstruct/` directory — build cache AND mtime database (see `cache.md`; the clear must work even when a database is corrupted, so it deletes the whole state dir)
- The mtime database can be deleted independently without affecting cached build outputs

## Combined input checksum

The `combined_input_checksum(inputs)` function computes a single hash representing all input files for a product. It:

1. Checksums each input file (using the two-layer cache)
2. Hashes the checksums as length-prefixed parts (not a separator join —
   see `hash_checksums`, which has an injection-proofing test)

Missing files get a `MISSING:<path>` sentinel so that different sets of missing files produce different combined checksums.

## See also

- [Fast `stat` on Linux](fast-stat.md) — notes on batching the per-input
  `fs::metadata` call (relevant for very large projects, cold cache, or
  network filesystems).
