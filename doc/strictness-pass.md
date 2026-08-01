# Feature design: another strictness pass — DONE

## Status

Implemented, then tightened again in a second pass (2026-08-01, below).
`clippy::pedantic` and `clippy::nursery` are `warn` at the crate root; all
clippy checks pass. The crate-level allow list is down from 36 to 12.

## Second pass (2026-08-01)

The 36 allows from the first pass were suppressing **560 real hits**. The
first pass had recorded per-lint counts as a to-do table; this pass worked
through it.

Result: **36 → 12 crate-level allows**, ~430 hits fixed, and the 12
survivors re-justified. Twelve more allows moved from the crate root down
to the specific site that needs them, which is the structural win — a new
hit of those lints anywhere else in the codebase is now a build error
instead of being silently covered by a blanket allow.

| Lint | Hits | Outcome |
| --- | --- | --- |
| `doc_markdown` | 248 | Fixed (autofix). Bare identifiers in doc comments → backticks. |
| `use_self` | 46 | Fixed (autofix). |
| `format_push_string` | 32 | Fixed by hand. `push_str(&format!(..))` → `write!`/`writeln!`, matching the form already used in graph.rs. |
| `or_fun_call` | 26 | Fixed by hand. 21 were `path.parent().unwrap_or(Path::new(".."))`; extracted `parent_dir` / `parent_dir_or_empty` helpers. |
| `map_unwrap_or` | 20 | Fixed (autofix). |
| `items_after_statements` | 16 | **Kept** — local `static REGEX: OnceLock` next to its `get_or_init`. |
| `option_if_let_else` | 15 | **Kept** — suggests `map_or`, which clippy's own `unnecessary_map_or` then flags. One site improved to `is_ok_and`. |
| `match_same_arms` | 25 | **Kept** — arms mean different things even when bodies coincide. |
| `unused_self` | 18 | **Kept** — signatures fixed by the trait. |
| `too_many_lines` | 14 | **Kept** — flat CLI dispatch tables. |
| `significant_drop_tightening` | 13 | **Kept** — unchanged policy from the first pass. |
| `needless_raw_string_hashes` | 12 | Fixed (autofix). |
| `cast_*` | 15 | **Kept** — known-small ranges, no untrusted input. |
| `implicit_clone`, `wildcard_imports`, `ref_as_ptr` | 10 | Fixed (autofix). |
| `struct_excessive_bools` | 7 | **Kept** — clap derives one bool per `--flag`. |
| `doc_link_with_quotes` | 7 | **Kept** — false positive on Rust array literals in doc comments. |
| `needless_pass_by_value` | 7 | **Kept** — trait signatures; alternative forces caller clones. |
| `case_sensitive_file_extension_comparisons` | 3 | Narrowed to 2 per-site allows (vim swap files are lowercase by definition). |
| `too_many_arguments` | 3 | Narrowed to per-site allows. Checked whether `discover_checker_products` could fold `dep_inputs` into `scan` — it cannot; `terms`/`zspell` pass computed lists. |
| `trivially_copy_pass_by_ref` | 2 | Fixed. `ProcessorType` is a fieldless `Copy` enum. |
| `permissions_set_readonly_false` | 2 | Narrowed to per-site. Cache objects are read-only; both sites widen the mode only to unlink on the next statement. |
| `similar_names` | 1 | Fixed. `proc`/`prov` in one scope → `processor`/`provenance`. |
| `option_option`, `struct_field_names`, `module_inception`, `naive_bytecount` | 4 | Narrowed to per-site allows with the reason at the decision. |
| `default_trait_access`, `needless_continue`, `if_not_else`, `single_match_else`, `unnested_or_patterns`, `ignored_unit_patterns` | ~15 | Fixed. |

### The 12 that remain

Each is now a decision with the alternative written out and rejected, not a
backlog entry. The rule recorded at the top of `src/main.rs`: a crate-level
allow requires that clippy's preferred form is not clearly better *and*
that the lint fires broadly enough that per-site allows would be worse.
Anything narrower belongs at the call site.

---

## Original pass

Implemented. `clippy::pedantic` and `clippy::nursery` are now `warn` at the
crate root. All clippy checks pass; all 529 tests pass. 36 explicit
per-lint allows remain, each with a category comment explaining why.

## Origin

`problems.txt`: "do a pass of making the code more strict (whenever we
relax strictness try to figure out if we can return the strictness)".

## Where the codebase already was, before this pass

Documented in `docs/src/internal/strictness.md`. Headline:

- `#![deny(clippy::all)]` and `#![deny(warnings)]` already in place.
- Only 6 surviving `#[allow(...)]` attributes, each named, scoped, and
  commented.
- `cargo clippy` clean against `clippy::all`.

I had initially reported "3 pedantic warnings, 0 nursery warnings" in an
earlier draft of this document — that was wrong. The original count was
an artifact of `deny(warnings)` causing clippy to stop early. The real
numbers, with pedantic and nursery enabled fresh, were: **1479 errors
across 58 distinct lints**.

## What this pass did

### Step 1: enable pedantic + nursery as `warn`

Added `#![warn(clippy::pedantic)]` and `#![warn(clippy::nursery)]` to
`src/main.rs`, immediately after the existing `deny`s. Because
`#![deny(warnings)]` would otherwise convert every new pedantic/nursery
warning to a fatal error, each firing lint got an explicit per-lint
`#![allow(...)]` to keep the build green.

### Step 2: peel off the high-volume autofixable lints

Worked top-down by occurrence count, removing each lint's `allow` and
running `cargo clippy --fix --allow-dirty`:

| Lint                                       | Occurrences | Result                                  |
| ------------------------------------------ | ----------- | --------------------------------------- |
| `uninlined_format_args`                    | 378         | Autofixed (everything → `{var}` form).  |
| `doc_markdown`                             | 305         | Allow kept — opinionated, noisy.        |
| `redundant_pub_crate`                      | 171         | Autofixed.                              |
| `redundant_closure_for_method_calls`       | 87          | Autofixed.                              |
| `missing_const_for_fn`                     | 77          | Autofixed.                              |
| `manual_let_else`                          | 51          | Allow kept — autofix wasn't safe.       |
| `format_push_string`                       | 32          | Allow kept — stylistic.                 |
| `or_fun_call`                              | 26          | Allow kept — stylistic.                 |
| `match_same_arms`                          | 21          | Allow kept — debatable.                 |
| `unused_self`                              | 20          | Allow kept — trait methods.             |
| `option_if_let_else`                       | 18          | Allow kept — `map_or` less readable.    |
| `map_unwrap_or`                            | 18          | Allow kept — `map_or_else` less readable. |
| `too_many_lines`                           | 15          | Allow kept — CLI dispatch matches.      |
| `items_after_statements`                   | 15          | Allow kept — local fns are fine.        |
| `significant_drop_tightening`              | 14          | Allow kept — needs per-site review.     |
| `single_match_else`                        | 13          | Allow kept — stylistic.                 |
| `unnecessary_wraps`                        | 12          | Allow kept — affects function signatures. |
| `needless_raw_string_hashes`               | 11          | Allow kept — regex constants.           |
| `needless_pass_by_value`                   | 10          | Allow kept — affects function signatures. |
| `derivable_impls`                          | 10          | Autofixed.                              |
| `cast_possible_truncation`                 | 10          | Allow kept — progress percentages.      |
| `implicit_clone`                           | 9           | Allow kept — cosmetic.                  |
| `struct_excessive_bools`                   | 7           | Allow kept — config types.              |
| `doc_link_with_quotes`                     | 7           | Allow kept — opinionated.               |
| `explicit_iter_loop`                       | 6           | Autofixed.                              |
| `collapsible_if`                           | 6           | Autofixed.                              |
| `cast_precision_loss`                      | 6           | Allow kept — same as cast_possible_truncation. |
| `unnecessary_literal_bound`                | 5           | Autofixed.                              |
| `stable_sort_primitive`                    | 5           | Autofixed.                              |
| `redundant_else`                           | 5           | Autofixed.                              |

Plus a long tail of single-occurrence lints, most fixed by later autofix
passes or allowed pending careful review.

### Step 3: hand-fix one orphan

`src/processors/generators/libreoffice.rs` had a 14-line doc comment
block, originally written for a `cleanup_marp_temp_dirs` function that
no longer exists in this file. The orphaned block sat above
`fn create_libreoffice` and was tripping
`empty_line_after_doc_comments`. Deleted the orphan; the function it
documented is no longer in this file, and the comment was a fossil.

### Step 4: hand-fix the to-do tier

Followed up by hand-fixing the rest of the lints clippy couldn't autofix.
Reduced 51 `manual_let_else` occurrences across 19 files to zero. Reduced
`unnecessary_wraps` from 12 to 0 (most: dropped the Result; two scaffolding
remote-pull functions and one PhaseHook signature kept Result with
per-fn `#[allow]` and a comment). Removed `useless_let_if_seq` (1),
`derive_partial_eq_without_eq` (1), `needless_collect` (1),
`needless_pass_by_ref_mut` (3 — both function signatures change to `&Command`
where Rust auto-derefs at call sites; one inner function), `crate_in_macro_def`
(1, autofix), `equatable_if_let` (1, autofix), and `bool_to_int_with_if` (1).

`significant_drop_tightening` was attempted but reverted to allowed: the
lint flags every guard whose scope extends past the last use, even by one
statement. In practice our guards are held for short cache lookups and
ad-hoc tightening produced busier code without a measurable contention
win. Marked as a deliberate policy allow in the source comment, not a
to-do.

### The 36 allows that remain

After this pass `src/main.rs` carries 36 explicit `#![allow(clippy::...)]`,
grouped into three buckets in the source:

1. **Numeric/cast lints** (3 allows). `cast_possible_truncation`,
   `cast_precision_loss`, `cast_sign_loss`. These fire dozens of times in
   places like progress percentage computation. Each fix would be a
   per-site `#[allow]` plus a comment, with no actual safety improvement
   (we're not casting untrusted values). Policy choice: keep allowed
   crate-wide.

2. **Stylistic / debatable** (32 allows). Lints where clippy's preferred
   shape is not obviously better than the original. Examples:
   `option_if_let_else` (`map_or` is often less readable),
   `match_same_arms` (kept distinct for readability), `unused_self` (trait
   methods that don't read self), `naive_bytecount` (would require adding
   the `bytecount` crate for one site), `case_sensitive_file_extension_comparisons`
   (vim swap files / `.tmp` are intentionally lowercase). Policy choice:
   keep allowed crate-wide.

3. **Mutex/lock guard tightening** (1 allow): `significant_drop_tightening`.
   See note above; deliberate policy allow.

## Net result

- **All clippy lints triggered by `pedantic` and `nursery` either fixed
  or explicitly allowed with a comment.** `cargo clippy --release` is
  silent.
- Approximately 830 occurrences fixed automatically (uninlined_format_args,
  redundant_pub_crate, redundant_closure_for_method_calls,
  missing_const_for_fn, derivable_impls, explicit_iter_loop, collapsible_if,
  unnecessary_literal_bound, stable_sort_primitive, redundant_else,
  crate_in_macro_def, equatable_if_let).
- Approximately 90 occurrences fixed by hand (manual_let_else × 51,
  unnecessary_wraps × 12, plus the smaller lints).
- One orphan doc block removed.
- All 529 tests pass.
- 6 → 36 `#[allow]` attributes — but every one is explicitly named at
  the crate root with a category comment, and the only "to-do" item left
  is `significant_drop_tightening`, kept as a deliberate policy allow.

## Items intentionally NOT done

These were considered and chosen against:

- **`.unwrap()` / `.expect()` audit**. Sampling showed all of them are
  principled (mutex locks, static regex literals, named-error invariants).
  Replacing them with `?` would propagate "should never happen" errors
  upward where the caller has no useful response.
- **`missing_docs`**. Would force `///` on every public item. Cosmetic
  for a binary crate.
- **`clippy::cargo`**. 23 warnings, all about transitive dep duplication
  driven by upstream Windows target crates. Not actionable.
- **`#![warn(unsafe_code)]`, `#![warn(unreachable_pub)]`,
  `#![warn(unused_crate_dependencies)]`**. Worth doing in a follow-up.
  Did not pursue in this pass to keep scope contained.

## Follow-up items

For the next pass, in priority order:

1. Add `#![warn(unsafe_code)]` to lock in zero-unsafe.
2. Add `#![warn(unreachable_pub)]` to catch over-exposed visibility.
3. Replace the `use crate::config::*;` glob in `src/builder/mod.rs`.
4. Decide whether to enable `clippy::pedantic` / `clippy::nursery` as
   `deny` instead of `warn` — depends on whether you want
   `#![allow(...)]` blocks or fresh hits to be a build break. With the
   per-lint allows already in place, flipping to `deny` would only
   affect *new* code introducing un-allowed lints, which is the right
   default.

## Open questions for the user

1. The "stylistic / debatable" allows (33 lints) are kept allowed because
   the alternate form is not obviously better. Want me to reconsider any
   specific one? `option_if_let_else`, `map_unwrap_or`, and
   `match_same_arms` are the highest-volume; for those, the policy is
   "human readability over clippy's preferred form." If you'd rather
   `map_or_else`/`map_or` be the house style, we can flip those.

2. Do you want me to keep going on the "to-do" tier — specifically the
   `manual_let_else` and `significant_drop_tightening` cleanups — in
   this same pass, or call it done here?
