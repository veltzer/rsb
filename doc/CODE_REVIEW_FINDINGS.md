# Code Review Findings

Full-codebase audit (2026-07-18). Checked off = fixed and verified by `cargo test` + `cargo clippy --all-targets`.

Status: all items fixed (one documented exception under item 31: slidev). Final state: 556 tests passing (131 unit + 425 integration), clippy clean under deny-warnings.

## Baseline

- [x] 0. `cargo clippy --all-targets` fails with 11 lint errors (deny-warnings) — fixed all lints (plus 2 warnings in tests/)

## Critical

- [x] 1. `-t` target builds lose all dependency ordering — fixed: `filter_by_targets` re-runs `resolve_dependencies()`; regression test `filter_by_targets_preserves_dependencies`
- [x] 2. `cmake` checker invokes non-existent `cmake --lint` — fixed: switched to `cmakelint` (pip); verified live with cmakelint 1.4.3
- [x] 3. `duplicate_files` checker is a silent no-op — fixed: discovery now emits ONE product spanning all scanned files (whole-set property); 3 integration tests incl. the incremental-add scenario

## High

- [x] 4. Hardlink restore chmods the shared cache inode writable — fixed: mode applied in `restore_file` per method; hardlinks never chmodded, exec-mode files restored via copy; unit tests
- [x] 5. `store_object` non-atomic write — fixed: unique temp file + rename; concurrent identical store treated as success
- [x] 6. Toggling `compression` desynchronizes store — fixed: compressed objects get `.zst` filename suffix; readers/restore key off actual on-disk format; round-trip test
- [x] 7. `max_jobs = 0` deadlocks the build — fixed: rejected at config-validation time with a clear error
- [x] 8. `[build]` section presence silently disables batching — fixed: config `batch_size` is now plain `usize` (0 = no limit); disable via CLI `-1` or per-processor `batch = false`; docs updated
- [x] 9. `--iset` can't target multi-instance processors — fixed: `rsplit_once('.')`; regression test with dotted iname
- [x] 10. Disabled processors still hard-fail the tool pre-flight — fixed: pre-flight skips `enabled = false` instances; regression test
- [x] 11. Watch mode exits on transient config error — fixed: construction errors reported and loop continues
- [x] 12. Same-stem C sources overwrite each other's `.o` — fixed: object paths mirror the manifest-relative source path; regression test; also fixed C++ link driver for separate-compile mode (was always `cc`)
- [x] 13. `pdfunite` passes non-PDF dep_inputs as pages — fixed: execute filters inputs to `.pdf` like ipdfunite
- [x] 14. Lua `CtxPtr` never cleared — fixed: cleared on every execute() exit path; out-of-execute calls get a clean Lua error instead of a panic/UAF

## Medium

- [x] 15. WordManager re-appends already-flushed words — fixed: flush drains pending into known set; failed flush now fails the product(s); unit test
- [x] 16. `words_file` not an input — fixed: aspell/zspell merge it into dep_auto at discovery, so dictionary edits invalidate cached checks by content
- [x] 17. aspell pipe deadlock — fixed: stdin fed from a writer thread while output drains; words-file read errors now fail processor creation (new `deserialize_and_try_create`)
- [x] 18. Watcher issues — fixed: watches registered before the initial build; registration failures always warn; ignored output dir derived from config (`output_dir`) instead of hardcoded "out". A single spurious rebuild after a real one remains possible and is cheap (all cache-skips) — deliberately not draining events, which could eat user edits made mid-build.
- [x] 19. Tags collect/loader mismatch — fixed: `tags.txt` loads as bare tags (the same convention collect writes)
- [x] 20. TagsConfig checksum excludes validation-rule fields — fixed: rule fields now in checksum_fields (only the CLI-only *_limit fields stay excluded)
- [x] 21. `trim()` GCs live blobs on unreadable descriptors — fixed: trim now hard-errors on unreadable/unparsable descriptors with the path in the message
- [x] 22. No content verification on blob restore/remote fetch — fixed: blob restore verifies first output's checksum like the Tree path; remote fetch re-hashes bytes before admitting (see #23); remote objects always stored raw so compression settings interoperate
- [x] 23. Remote FileBackend writes non-atomically to shared mount — fixed: temp + rename in dest dir; also removed now-dead `download()` from the RemoteCache trait; fetch verifies checksum before admitting bytes into local CAS (covers the poisoning half of #22)
- [x] 24. Config plumbing — fixed: user/CLI-set output paths never rewritten and prefix match respects `/` boundary; nested `[vars]` refs fully resolved order-independently (cycles error); `[vars]` lines blanked instead of deleted so provenance line numbers stay exact; unit tests for all three
- [x] 25. Silent skips — fixed: explicit glob errors now hard-error; `collect_dirs_with_ext` errors on unreadable dirs; Lua auto_detect/required_tools failures warn loudly (trait returns can't propagate); a2x/linux_module fail when the declared output wasn't produced; linux_module fails when `uname -r` is unavailable instead of a literal `$(uname -r)` path; module clean failures now reported
- [x] 26. Dead/ignored config fields — fixed: jekyll/jinja2/mako honor `command` (defaults added: jekyll, python3×2); script `fix_command` reachable via new instance-level `config_has_fix()`; hadolint has a real binary recipe (new `ArchiveKind::Raw`)
- [x] 27. Batch checkers — fixed: svgo uses `-o -` and batch disabled (svgo requires matching in/out counts); all 10 native checkers now return per-file results via `execute_checker_batch_per_file`; marp_images regex stops the path capture before whitespace
- [x] 28. libreoffice lock path — fixed: per-user lock in the system temp dir (per-user is the correct scope — one LibreOffice profile per user; also honors TMPDIR)
- [x] 29. `symlink_install.rs` unix-only symlink — fixed: `platform::symlink_file` wrapper with cfg guards in platform.rs

## Low

- [x] 30. Bare `?` on IO without context — fixed everywhere: main.rs, graph.rs (`to_svg` also no longer leaves a zombie `dot` on write failure; same for `dot_to_svg`), webcache.rs, processors/mod.rs (flush_words, clean_outputs), terms.rs ×4, tags.rs ×4, tera.rs ×3, linux_module.rs; `create_all_default_processors` now returns `Result` with per-processor context instead of `.unwrap()`
- [x] 31. Rule nits — fixed: `search_processors` uses static plugin metadata only (no instances); pdflatex `-shell-escape` is now the `shell_escape` config field (default true, in known/checksum/descriptions); scalar `[processor]` entries hard-error at validation; `src_dirs = []` (and any empty-string element) rejected; `${...}` in comments no longer fails config load; `cache_output_dir` added to field_descriptions for cargo/sphinx/mdbook/npm/gem. EXCEPTION — slidev's undeclared `dist/` output is a design issue (checker should become a generator with declared outputs); left as a documented limitation rather than a speculative change to slidev CLI flags.
- [x] 32. Robustness edges — fixed: `.tools.versions` written via temp+rename; bare-`X` version matching added; checksum combining is length-prefixed (no `:` ambiguity); mtime entries for files modified <2s ago are not persisted (git-style racy-timestamp guard); tree descriptors sorted by path (read_dir order no longer causes spurious changes); remote upload temp files use create_new (no symlink pre-plant) and FileBackend renames into place; `add_virtual_files` no longer binary-searches a vec it's mutating; deps-cache checksums are taken before the scan; `NO_COLOR=` (empty) keeps color per spec; exit code 5 reachable (IO errors in the chain classify as IoError); bash-completion injection failures now warn with the missed target; the cwd-mutating test uses `DepsCache::open_in` instead of `set_current_dir`; empty `src/lib.rs` deleted
