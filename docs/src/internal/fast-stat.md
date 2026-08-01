# Fast, Scalable `stat` on Linux

Background notes on batching `stat`-style metadata calls. Relevant to
rsconstruct because `combined_input_checksum` (see [Checksum
Cache](checksum-cache.md)) calls `fs::metadata` once per input file to read
the mtime before consulting the persistent cache. For projects with tens of
thousands of products, that's tens of thousands of serial `statx` syscalls
per build. This doc records what's available on Linux, when it actually
matters, and what a Rust implementation would look like — so the option is
on file when profiling shows the stat path becoming the bottleneck.

## Is there a `stat(2)` syscall that gives info about lots of files?

No. There is no batch `stat(2)` on Linux. Each call covers exactly one
path. The closest options:

- **`statx(2)`** (Linux 4.11+): richer info than `stat`, still one file per
  call.
- **`getdents(2)` / `readdir(3)`**: returns directory entries in bulk. On
  most filesystems each entry includes `d_type` (file/dir/symlink), so you
  can classify without a `stat` call. But you don't get size, mtime, or
  permissions — for those you still need per-file `stat`.
- **`fstatat(2)` with a dirfd**: avoids re-resolving the parent path
  repeatedly when scanning one directory tree. Still one syscall per file.
- **io_uring** (Linux 5.6+): submit many `statx` ops in one syscall and
  reap them together. Not a "batch stat" in the API sense, but in practice
  it gives the throughput win — fewer syscall round-trips, async
  completion. This is what you want for "stat 100k files fast."
- **BSD `getattrlistbulk(2)`** (macOS/FreeBSD): genuinely returns
  stat-like info for many entries in one call. Linux has no equivalent.

For "I need stat data for many files on Linux", the standard answer is
`getdents` for what you can get cheaply, then `io_uring` with
`IORING_OP_STATX` for the rest.

## What io_uring actually gives you

You submit N `statx` operations to a ring buffer, the kernel processes
them (often in parallel), and you reap N results. From userspace it can be
just **2 syscalls total** regardless of N — one `io_uring_enter` to
submit/wait, one to drain completions. With `SQPOLL` mode it can be
**zero syscalls** in steady state.

It is not a single `stat_many(paths[], results[])` call. The kernel still
does one `statx`'s worth of work per file internally — it just doesn't
make you pay the syscall-boundary cost for each one.

Empirical speedups:

- Cold cache (real disk I/O): big win — kernel parallelizes the lookups,
  ~5–10× over serial `statx`.
- Warm cache: smaller win (~2×), since each `statx` was already cheap; you
  are mostly saving syscall overhead.
- Submission queue depth matters — keep it deep (256–4096) to let the
  kernel reorder.

For 1M files: serial `statx` is ~1M syscalls. With io_uring at ring depth
4096, it's ~244 `io_uring_enter` calls, or zero with SQPOLL. The
syscall-count saving is real but in most workloads parallelism is the
bigger deal — they happen to come from the same mechanism.

## When it actually matters

| Workload | Serial cost (5,000 files) | io_uring win |
|---|---|---|
| Warm cache (dentries/inodes in memory) | 2–5 ms | 1.5–2× |
| Cold cache, SSD | 250 ms – 1 s | 5–10× |
| Cold cache, HDD | 5–10 s | ~10× |
| Network filesystem (NFS, SMB) | 5 s+ (RTT-bound) | 5–10× |

Warm cache: each `statx` is ~0.5–1µs. The absolute time is small enough
that the code complexity probably is not justified.

Cold cache or network filesystem: io_uring is genuinely worth it.

## Simpler alternatives to try first

1. **Threaded parallel `metadata()`** — `rayon::par_iter().map(fs::metadata)`
   with ~16 threads. Far less code than io_uring, competitive on warm
   cache. On cold cache it is worse than io_uring (threads still block one
   syscall at a time) but still 5–10× faster than serial.
2. **`getdents` + `d_type`** — if all you need is "is it a file/dir/symlink"
   and not size/mtime, scan a directory and get type info essentially for
   free. This is already what `ignore::WalkBuilder` does in
   `src/file_index.rs`.
3. **`fstatat` with a dirfd** — if statting many files in the same
   directory, opening the dir once and using `fstatat` shaves
   path-resolution cost. ~1.5× over plain `stat`.

## Rust options

Three realistic choices:

### `tokio-uring`

Easiest if already using Tokio. As of writing, `tokio-uring` does not
expose `statx` as a first-class op in all versions — may need to drop to
raw ops.

### `rio`

Simple, blocking-style API over io_uring. Less maintained but works.

### `io-uring` crate (direct binding)

Most control, most code:

```rust
use io_uring::{IoUring, opcode, types};

let mut ring = IoUring::new(4096)?;
let mut results: Vec<libc::statx> = vec![unsafe { std::mem::zeroed() }; paths.len()];

for (i, path) in paths.iter().enumerate() {
    let sqe = opcode::Statx::new(
        types::Fd(libc::AT_FDCWD),
        path.as_ptr(),
        &mut results[i] as *mut _ as *mut _,
    )
    .mask(libc::STATX_BASIC_STATS)
    .build()
    .user_data(i as u64);

    unsafe { ring.submission().push(&sqe)?; }
}

ring.submit_and_wait(paths.len())?;

for cqe in ring.completion() {
    let idx = cqe.user_data() as usize;
    if cqe.result() < 0 {
        // -errno
    }
    // results[idx] is filled
}
```

For 1M files, loop: submit ~4096, wait, drain, repeat. Don't try to push
all 1M into the ring at once — the SQ has finite size.

### Pitfalls in Rust

- **Path lifetimes**: the kernel reads the path string asynchronously. The
  `&CStr` passed must outlive the operation. With `tokio-uring` this is
  handled by ownership transfer; with raw `io-uring` you must keep buffers
  alive until the CQE arrives. Easiest pattern: a `Vec<CString>` that
  lives for the whole batch.
- **`statx` buffer**: same — must be valid until completion. Pre-allocate
  `Vec<statx>` of the right size.
- **Runtime setup**: `tokio-uring` requires explicit `tokio_uring::start`;
  it is not a drop-in replacement for `tokio::main`.
- **Linux vs macOS**: io_uring is Linux-only, and macOS is a supported
  target, so this is the one place a real platform split would be needed.
  Any usage must live behind `src/platform.rs`, which owns every OS call
  (see [Coding Standards](coding-standards.md)). The macOS fallback would
  be parallel `fs::metadata`.

## Relevance to rsconstruct

Today, `combined_input_checksum` in `src/checksum.rs` calls `fs::metadata`
serially per input file to read mtime. The latest profiling run (see
[Profiling](profiling.md)) shows `statx` at ~0.19% of CPU on a 10k-product
project — not currently a bottleneck.

This becomes interesting when:

- A project has 100k+ products, or
- Builds run on cold cache (CI without warm dentries), or
- Builds run against a network filesystem.

Recommended sequence if this ever becomes the hot path:

1. Confirm via profiling that `statx` / `fs::metadata` actually dominates
   (not graph construction, not hashing, not subprocess spawn).
2. Try `rayon::par_iter` + `fs::metadata` first — minimal code, big enough
   win for most cases.
3. Only if step 2 is insufficient and the workload is cold-cache or
   network-FS bound, reach for `io-uring` behind `platform.rs`.

## Code-scan: how easy would the integration be today?

A scan of every `fs::metadata` / mtime call site in `src/` (recorded here
so future work doesn't have to redo it) showed the following:

### Where mtime is read

**Hot path — would benefit from batching:**

- `combined_input_checksum(ctx, &product.inputs)` in `src/checksum.rs` —
  one `fs::metadata` per input via `fast_checksum`. Called from four
  sites, all per-product:
  - `src/builder/build.rs` (status pass — `for product in products`)
  - `src/executor/mod.rs` (planning pass — `for &id in order`)
  - `src/executor/execution.rs` (execution pass — `for &id in level`)
  - `src/builder/product.rs` (single-product diagnostics)
- `checksum_fast(ctx, source)` in `src/deps_cache.rs` — one stat per
  (analyzer, source) pair from inside `get` / `classify` / `set`.

**Incidental — not worth batching:**

- `src/object_store/{management,descriptors,blobs,operations}.rs` —
  single-file metadata reads (permission bits, sizes for cache stats).
  Low volume, no batching opportunity.
- `src/platform.rs` — single permission read.

### Why a swap is harder than it looks

1. **Call sites are per-product, not per-batch.** Every hot caller has
   the shape `for product in products { combined_input_checksum(ctx,
   &product.inputs) }`. The function only ever sees one product's 1–10
   inputs. To get io_uring's win you need to feed it
   hundreds-to-thousands of paths at once, which means **inverting the
   loop**: collect every input across every product first, batch-stat
   them, then do the per-product classification using the pre-populated
   cache. That's a structural refactor of the status / planning /
   execution loops, not a one-line change in `checksum.rs`.

2. **The mtime cache is read mid-`fast_checksum`.** Today it does: stat
   → read mtime DB → compare → maybe read file → maybe write DB. Batching
   the stat means splitting this into phases: (1) batched stat for all
   paths, (2) batched mtime DB read, (3) per-path decide-and-rehash.
   Doable, but it changes the function's contract (no longer "pure
   per-path") and shifts the access patterns of the in-memory
   `checksum_cache` and `mtime_db`.

3. **Result-shape change ripples into stats.** `ChecksumPath::MtimeShortcut`
   vs `FullRead` is reported per call and feeds deps cache stats in
   `src/builder/mod.rs`. A batch API needs to return a parallel
   `Vec<ChecksumPath>` and callers need to attribute results back to
   products.

4. **Linux/macOS constraint.** Per `CLAUDE.md`, every OS call lives in
   `src/platform.rs`. The io_uring bits would be a **platform shim** —
   a thin wrapper like `pub fn batch_mtimes(paths: &[&Path]) ->
   Vec<io::Result<(i64, u32)>>` whose only job is to expose one OS-agnostic
   API to callers, with a Linux impl using the `io-uring` crate and a
   non-Linux fallback (serial `fs::metadata` or rayon). Callers stay
   clean; the `#[cfg]` lives in exactly one place. This part of the
   refactor is mechanical — the abstraction boundary already exists
   (see `platform::set_permissions_mode` for the existing pattern).

5. **Deps cache has no batch to give it.** `deps_cache::get` is called
   one (analyzer, source) at a time from inside the analyzer scan loop.
   Batching it would require pre-collecting all sources for an analyzer
   before scanning — another structural change. Probably leave this on
   the serial path for v1.

### Sketch of a realistic refactor

- Add `platform::batch_mtimes(paths: &[&Path]) -> Vec<io::Result<(i64,
  u32)>>` (Linux: io_uring; else: serial or rayon).
- Add `checksum::warm_mtime_cache(ctx, paths: &[PathBuf])` that calls
  `batch_mtimes`, reads the mtime DB once for all paths, and
  pre-populates `ctx.checksum_cache` for any path whose mtime matches
  the cached entry.
- Modify the three hot loops (`builder/build.rs`, `executor/mod.rs`,
  `executor/execution.rs`) to call `warm_mtime_cache` once with the
  union of all `product.inputs` before entering the per-product loop.
  The existing `combined_input_checksum` then mostly hits the in-memory
  cache and skips the per-file stat entirely.
- Leave deps cache on the serial path for v1.

**Effort estimate: medium.** Roughly 200–400 lines, the bulk of which
is the loop inversion in the three hot files plus the platform shim.
The actual io_uring code is small (~50 lines). Existing
`combined_input_checksum` tests would mostly still apply since the
per-product API can stay unchanged.

### Verdict

Not worth doing today. Profiling shows `statx` at ~0.19 % of CPU on the
10k-product `teaching-slides` project; this refactor would buy almost
nothing on warm cache, which is the common case for incremental builds.
It only pays off on cold-cache CI, network filesystems, or 100k+
products. If we ever hit one of those, start with `rayon::par_iter` —
it would deliver most of the win for ~50 lines and no platform shim.

## See also

- [Checksum Cache](checksum-cache.md) — describes the mtime-stat path that
  would benefit from this.
- [Profiling](profiling.md) — current stat cost measurements.
- [Suggestions](suggestions.md) — tracking entry for this optimization.
