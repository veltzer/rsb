# Architectural Assessment

Full-codebase architecture review, 2026-08-05. This replaces the retired
`ARCHITECTURE_FINDINGS.md` (a bug-level audit whose items were remediated
through 2026-08-04; its record lives in git history). This document is
deliberately different in kind: it records **fundamental architectural
deficiencies** — properties of the design that generate whole classes of
bugs — not individual defects. The bug-level findings that led here are
cited as *symptoms as evidence*, not as the point.

Method: five parallel deep scans through distinct lenses (module dependency
topology; build-pipeline data flow and state lifecycle; the processor
extension model; the persistence/caching model; the command/driver layer),
followed by hand-verification of the load-bearing claims. Scale at time of
review: 169 `.rs` files, ~39,000 lines, 33 flat top-level modules, single
binary crate, ~93 processors.

The six deficiencies are ordered by how much of everything else each one
causes. §1 and §2 are generators — most recorded bug history is downstream
of them. §7 explains the mechanism and ranks the fixes by leverage.

---

## 1. Per-processor knowledge has no single source of truth — and that IS the module tangle

### The claim

A processor is the system's unit of extension. Describing one completely
requires **~14 edit sites across 8+ files**, of which the compiler enforces
roughly 4. There is no schema from which the rest derive; each site is a
hand-maintained projection of the same facts.

### The evidence

The touch-points for one processor:

1. The processor file itself + `inventory::submit!`
2. The `mod` declaration in its category's `mod.rs` — **a silent kill
   switch**: omit it and the file compiles as dead code, submits nothing,
   and the processor simply does not exist, with no diagnostic
3. Config struct in `config/processor_configs.rs` (if any custom field)
4. `#[serde(default = "fn")]` + free `default_*()` functions — default
   value encoding #1
5. `impl Default` — default value encoding #2 (same values, second syntax)
6. `known_fields()` — field name list #1
7. `checksum_fields()` — field name list #2
8. `must_fields()` — field name list #3
9. `field_descriptions()` — field name list #4
10. `scan_defaults_for` in `config/mod.rs` — a **92-arm** name-keyed match
11. `processor_defaults_for` in `config/mod.rs` — a **71-arm** match —
    default value encoding #3
12. `expected_field_type` in `config/mod.rs` — a **~120-arm** match on
    `(processor, field)`
13. `TOOLS` registry in `src/tools.rs` (83 entries)
14. Docs page, `SUMMARY.md` entry, test file, `tests/main.rs` registration

Every config field is therefore declared up to **six times** (struct+serde,
Default impl, known/checksum/description lists, type table), and every
default value **twice in two encodings** (Rust `Default` for serde; TOML
data in `processor_defaults_for` for the provenance system). `MarpConfig`
carries an in-source comment admitting the two encodings are
hand-synchronized *and that a divergence already regressed once*. The
14-name `StandardConfig` field block is copy-pasted across ~36
`KnownFields` impls.

The match tables' `_ => None` fallthrough is a **legal, silent answer** — a
missing arm is indistinguishable from a deliberate "no defaults here"
(which is correct for `script`, `creator`, `explicit`). So the compiler
cannot help, and git history proves the cost is not just high but
*unpredictable*: commit `fbaa7ac` added `prettier` in exactly one file — it
compiled, linked, registered, and shipped **silently mis-configured** (no
scan defaults, no processor defaults, no TOOLS entry); the remaining
registries were filled in by later commits, discovered by symptom.

The system's own tests confess the diagnosis. `config/tests.rs` carries
~150 lines of whole-registry consistency checks
(`every_known_field_has_an_expected_type`,
`every_expected_type_arm_is_a_live_field`, …) — **a schema checker written
by hand, at runtime, in the test harness, because there is no schema**. One
of them must verify its property backwards, with the comment: *"The table
is a match, not data, so it can't be enumerated directly."*

### Why this is also the dependency knot

The crate's central cycle — `config → registries → processors → config` —
exists *because of* this duplication. Measured by reference sites:
processors→config 679, processors→registries 638; `config/mod.rs:483`
imports the registry while `registries/processor.rs` imports both
`processors` (for the `create` fn pointer and `ProcessorType`) and `config`
(for `KnownFields`). The 163 hardcoded processor-name arms in `config` are
per-processor knowledge that could not be expressed on the plugin entry, so
they landed in `config`, which therefore must know the registry, which must
know processors, which must know config. **These three modules
(~7,500 lines: the config schema, the registry, all 93 tool integrations,
and the process runner) are one indivisible unit** — a workspace split is
impossible until this is resolved. The docs' claim ("no central registry
table to update", `architecture.md:323`) is true of the construction table
`inventory` eliminated and false of the three metadata tables that
replaced it.

### The honest caveat

Two of the project's own rules feed this deficiency:

- **"No macros"** blocks the `#[derive]`-style unification that would let a
  single per-field `FieldSpec { name, type, default, affects_output,
  required, doc }` table generate all six projections. Without a derive,
  the spec table cannot generate struct fields and serde attributes; you
  either keep both (partial win) or lose static typing at every
  `self.config.x` site.
- **"Never create dummy instances"** forces metadata onto the static plugin
  entry, which is what pushed the config-dependent knowledge into `config`'s
  match tables (see also §3).

Both rules are defensible. Their combined price is this deficiency, and the
price is currently paid silently.

### Fix shape

The H1-shaped refactor: a data/constructor split of the plugin entry that
absorbs the three match tables into the plugin file (defaults and scan
defaults as `toml::Value` data — the provenance system needs data anyway),
plus deriving `known_fields` by union with `StandardConfig`, flipping
`checksum_fields` to in-by-default/opt-out, and — cheap, immediate,
schema-less — **a processor-level completeness test**: iterate
`all_plugins()` and assert each has a defaults arm or explicit opt-out, a
docs page, a `SUMMARY.md` entry, and a test file. That one test closes the
`prettier` failure mode today.

> **STATUS (2026-08-05): DONE.** A processor is now one file. `FieldSpec`
> on the plugin entry is the schema (known/checksum/must fields,
> descriptions, and expected types all derive from it); the three match
> tables are deleted; every default value is encoded exactly once (the
> marp double-encoding class is structurally gone); config structs live in
> their processor files (`processor_configs.rs`: 1,996 → 281 lines, now
> only `StandardConfig` + shared types + analyzer configs); the tool
> registry accepts per-file `ToolInfo` submissions; completeness tests
> enforce the mod declaration, docs page, and test file (with allowlists
> for grandfathered gaps). The five hand-written schema-consistency tests
> were deleted as structurally impossible to violate. Remaining from this
> section: the `config↔registries↔processors` import cycle still exists
> (the *knowledge* moved; the module boundaries didn't), and docs pages
> are still hand-written — the config tables in them can drift until
> generated or verified.

---

## 2. Facts are re-derived instead of passed — the build has no materialized identities

### Input content identity is never data

`Product.inputs` is `Vec<PathBuf>` — a list of *names*. The build's answer
to "what content is in these files" exists only as transient `String`s
recomputed from live disk **three to five times per product per build**:

1. `classify_products` (`executor/mod.rs`) — stored in
   `ClassifiedProduct.input_checksum`
2. `unlink_pending_outputs` → `remove_stale_outputs` — uses checksum #1 to
   select files for **deletion**
3. `prepare_level_work` (`executor/execution.rs`) — recomputes, with a
   15-line comment on why reusing #1 is wrong
4. `handle_restore` — uses #3
5. `handle_success` — recomputes **again** post-execution, with a 14-line
   comment on why reusing #3 is wrong

Each site is individually correct and each comment genuinely illuminating.
Together they state what the code cannot: **the descriptor key is not a
property of a product; it is a property of a product at an instant**, and
the system needs three different instants (pre-unlink, pre-execute,
post-execute) to be correct — with nothing but comments to tell a
maintainer which instant they are standing in. The doc comment on
`Executor::execute` claims checksums are reused from classification; the
same file recomputes them 600 lines later. Nobody owns the question.

Corollary machinery that exists solely to keep independent observations of
one fact in agreement: the `forget_in_session` eviction protocol (three
manual call sites), the separate `checksum_output` function (one cache
serving two populations — stable inputs, volatile outputs — distinguished
only by which function the caller picked), the 2-second
recently-modified mtime heuristic, and the `MISSING:{path}` sentinel — an
absence marker that is a perfectly valid `String`, type-indistinguishable
from a real content hash, flowing into `descriptor_key()`.

### Cache-key completeness is unowned

`CacheKey` (typed components, length-prefixed, attributable) solved key
*composition* — but contribution remains fully distributed: Config at
`Product::new`, Variant at `with_variant`, Analyzer pieces mid-pipeline,
ToolVersion via `apply_tool_version_hashes` — **a post-hoc mutation pass
over the whole graph that any caller can skip undetected**.
`CacheKey::new()` is a valid, empty, under-keyed key, indistinguishable
from a legitimately component-free product. That is exactly how the
"tool upgrades leave stale PASSes valid" bug hid: an absent contributor is
invisible. Component *order* is documented as significant and enforced by
nothing — reordering two calls in `build_graph_with_processors_impl`
silently cold-starts every cache in every project.

### Product state has no representation

There is no `ProductState`, no written state machine. Four overlapping
enums exist (`ProductAction`, `ExplainAction`, `PreCheckResult`,
`RestoreOutcome`) plus the actual runtime state: membership in
`failed_products`/`failed_processors` HashSets, where "skipped because a
dependency failed" and "failed during execution" are the same state and are
counted together. `Classification` — the phase output shaped like a plan —
is a checkpoint the executor refuses to honor: its `input_checksum` field
is written and never read, its `action` is consumed as a boolean, and
`prepare_level_work` re-derives the rebuild decision by calling the object
store directly, **bypassing the `BuildPolicy` trait that was extracted to
own that decision**. Consequently `status`, `--dry-run`, the
"`N to build`" banner, and the executor answer "what will happen to this
product" through three independent code paths — **the plan shown to the
user is structurally not the plan that executes.**

### The graph undermines its own indices

`BuildGraph` owns four indices derived from product contents (`interner`,
`output_to_product`, `input_to_products`, `checker_dedup`) and also hands
out `get_product_mut`. Analyzers extend `product.inputs` through it;
the indices are never updated. `resolve_dependencies` then does
`interner.get(input)?` — the `?` **silently skips un-interned paths**, so
an analyzer-added input that is another product's output produces no
dependency edge and no diagnostic. Graph-structure invariants were
progressively hardened into construction (`register_product`, output
conflict checks — good, deliberate work); the mutable back door walks
around all of it on every build.

### Fix shape

`CacheKey` is the in-repo proof of the cure — it turned an opaque string
assembled at five uncoordinated sites into typed, ordered, attributable
data. Apply the same move three more times: a `ResolvedInputs` value
(path → `InputDigest::{Content(hash), Absent}`) produced once per product
at a defined instant and passed forward; a `CacheKey` that cannot be
`digest()`ed until every contributor has had its turn (typed builder /
completeness token, components appended in `Component` order which is
already `Ord`); a single `ProductState` consumed by classify, executor,
`status`, and `--explain` alike. Seal `get_product_mut` behind an
`add_inputs(product_id, paths)` method that maintains the indices.

---

## 3. Capabilities live on the link-time registry entry, so the only user extension path is structurally crippled

`supports_batch`, `can_fix`, `max_jobs_cap`, `version`, `is_native`, and
all four field-metadata accessors are `&'static` fields on
`ProcessorPlugin`, registered via `inventory::submit!` at link time.
Anything not compiled into the binary therefore **cannot have
capabilities**. Lua plugins — the only extension mechanism available to a
user who cannot rebuild rsconstruct — get the execution trait and none of
the metadata registry:

| Capability | Native | Lua |
|---|---|---|
| discover / execute / clean / auto_detect | yes | yes |
| batch execution | yes | no |
| fix | yes | no |
| max_jobs cap | yes | no |
| **cache versioning** | yes | **no — `processor_version` → None → v0** |
| field validation / known_fields | yes | no |
| multi-instance config sections | yes | no |
| defconfig / list / search / clap completion | yes | no |

The versioning row is a live correctness hole: editing a Lua plugin's logic
does not invalidate its cached outputs (the script is in no input list and
the version is a constant 0). Two more structural consequences:

- The trait's fallibility model cannot host a scripting runtime:
  `auto_detect` and `required_tools` return non-`Result` types, so the Lua
  host downgrades real errors to `eprintln!` warnings — a direct violation
  of the project's own strictness rule, with comments showing the author
  knew and had no alternative.
- The limitation bites natively too. The `script` processor declares
  `can_fix: false` beside a working `fix()` implementation, reconciled by
  `registries::can_fix(name) || processors[name].config_has_fix()` at the
  call site — the static flag can only lie about config-dependent
  capabilities. Every capability now exists as a flag/method pair, and the
  flag lives on the registry for some capabilities, the trait for others,
  and both for two of them, with nothing naming the boundary.

This is not neglect; it is forced by pairing link-time registration with
the no-dummy-instances rule. Link-time registration itself is a good trade
for this tool (no central construction table, metadata without
instantiation, whole-registry tests, static clap completion) — but it
concentrates all its cost in one place: **native extension is compile-time
only, so Lua is the real user extension surface, so the Lua capability gap
is the model's most consequential property.**

### Fix shape

A capability descriptor the processor supplies at registration **or** at
runtime — a synthetic `ProcessorPlugin` built at Lua discovery time
(version = content hash of the script, closing the cache hole; batch / fix
/ cap / fields from optional Lua globals) — so `find_plugin` never returns
`None` for a live processor and capability queries have exactly one home.
Generalize `config_has_fix` into the pattern for all config-dependent
capabilities instead of an ad-hoc `||` per capability.

---

## 4. There is no command layer — fifteen drivers hand-roll subsets of one pipeline

There is exactly one canonical pipeline (discover → analyze → resolve →
validate → classify → execute) and **fifteen command entry points** that
re-implement prefixes of it by hand: three independent discovery loops
(`build_graph_with_processors_impl`, `build_graph_filtered`, an inline copy
in `analyzers build`), three independent "will this rebuild?"
implementations (executor policy, `print_product_status`,
`valid_cache_keys`), two execution loops (`Executor::execute`, `fix`'s own
batch/single dispatch), and one hand-rolled filesystem walk
(`clean unknown`). The drift is documented in the code itself: the
`analyzers build` copy shipped skipping resolve+validate, reported success
on configs `build` rejects, was patched — and remains a copy.

The sharpest evidence that this is architectural: **interruptibility is a
property of the executor, not of the process.** `is_interrupted()` polling
lives in `executor/` (plus watch and the subprocess runner). `fix`,
`clean unknown`, and `analyzers build` poll it zero times — any driver that
skips the executor silently loses Ctrl+C.

`Builder` is the non-abstraction at the center: three fields (`config`,
`file_index`, `object_store`), **31 public entry points across 10 files**,
no build state, everything re-derived per call (`create_processors()` at 14
sites). It is a service locator wearing a struct — `Builder::doctor()`,
`Builder::sloc()`, `Builder::product_show()` do not build anything. The
dispatch knowledge ("which commands need config") is split three ways:
20 hand-placed `Builder::new` calls in `main.rs`, ad-hoc
`rsconstruct.toml` existence probes in three places, and
`unreachable!("handled in main.rs")` arms inside `Builder` methods that
take whole action enums and declare half the variants impossible — a
runtime panic standing in for a compile-time property.

The testability receipt: `Builder::new` performs three unmockable side
effects (require config in CWD, open redb, walk the filesystem), with no
constructor accepting pre-built parts. **`src/builder/` has 5 unit tests
against 31 entry points** (all for one pure free function), versus 67 in
`src/config/`. All behavioral coverage is integration tests spawning the
binary. The architecture is selecting which code gets tested.

Output is the same absence viewed from the other side. `output.rs` is a
correct reporter with an honest charter, enforced by a scanner test — over
a **hardcoded 9-file allowlist** (`executor/`, `object_store/`). Outside
it: ~360 raw `println!` sites, 73 `is_json_mode()` call sites (versus 10
uses of the correct combined predicate), and one fact — "product X
failed" — crossing **eight independent formatting/suppression decision
points** between the executor and the terminal. Two output regimes coexist
in one binary, and the guard test is green precisely because the broken
half is outside its file list (`status --json --verbose` and
`watch --json` both corrupt the JSON stream today). Mode flags are threaded
by four different mechanisms (`OnceLock` globals, a hand-threaded `verbose`
parameter in 72 signatures, `BuildOptions` fields, a `BuildContext`
setter), and `--quiet` appears in zero tests.

### Fix shape

Not more extraction — `cache_cmd.rs`'s header records that extracting the
biggest match arm just relocated the pattern. Two structural moves:

1. **A `Command` abstraction** owning config acquisition, pipeline access,
   interrupt polling, and result rendering, so the fifteen drivers become
   fifteen policies over one driver and a sixteenth cannot silently opt out
   of Ctrl+C. Prerequisite: `Builder` (or its successor `Project` value)
   accepts its three dependencies instead of constructing them, which is
   the single change that makes the command layer unit-testable.
2. **A data/presenter split**: commands return a serializable result value;
   one presenter chooses json / verbose table / compact table / suppressed.
   This deletes the copy-pasted `if json {…} else if verbose {…} else {…}`
   triple at ~30 sites, defines `--json --verbose` for free, and lets the
   bare-println scanner become universal instead of allowlisted.

---

## 5. There is no persistence layer — seven stores share a directory name and nothing else

`.rsconstruct/` is a naming convention, not a layer. Seven mechanisms
(`objects/`, `descriptors/`, `db.redb`, `mtime.redb`, `deps.redb`,
`webcache.redb`, plus out-of-directory state: `.tools.versions`, words
files, tags state) have no common open/verify/repair lifecycle, no shared
path constants, and **four mutually contradictory corruption policies**:

- `cache trim` **fails closed** on an unparsable descriptor (correct in
  isolation — skipping would GC live blobs);
- `get_descriptor` **fails open silently** on the same descriptor (a
  permanent, invisible cache miss);
- `db.rs` **fails loud** ("run cache clear") and, despite its function
  name, never recreates;
- a corrupt `.tools.versions` **fails the whole build**.

Net effect: one corrupt descriptor makes `build` silently rebuild forever
while `cache trim` refuses to run, and the only repair is `cache clear` —
`remove_dir_all` on the whole directory, destroying five unrelated stores.
The schema-versioning inversion completes the picture: the **descriptor
format — the on-disk and over-the-wire contract shipped between machines
via the remote cache — has no version field**, while `webcache`, the one
freely-re-fetchable store, is the only versioned one (`webcache_v2`).
Symptomatic of the missing inventory: `config/mod.rs` documents a
`toolver.redb` that does not exist.

**Concurrency is unresolved in both directions.** The design is
single-process by enforcement (four redb exclusive locks; a test-only
`db_name` parameter exists because two stores cannot open one directory)
yet pays multi-process costs throughout (pid-tagged temp names,
rename-race recovery branches, uuid tempfiles in the remote backend) —
and one function's comment justifies including temp files with a
single-process argument fifty lines from pid-tagging that assumes the
opposite. Committing to single-process (one flock at startup) deletes real
code from the two hottest write paths; committing to multi-process unlocks
daemon mode and shared CI caches. The current position pays for both and
guarantees neither.

**The remote cache is a transport, not an abstraction.** `RemoteCache` is
four methods with zero pinned semantics: error-vs-absence is decided per
backend (HTTP rigorously distinguishes them; S3's `exists` folds
credential failure into "absent"; File folds permission errors into
"absent"), atomicity is per-backend folklore, and there is no conformance
suite — `HttpBackend` has zero tests, and the good round-trip tests all
run against `FileBackend` only. The write-only-remote and torn-write bugs
fixed in 2026-08 could exist *because* nothing pins the contract.

### Fix shape

A `Store` trait with declared schema version and an open/verify/repair
lifecycle; one policy for absence, one for corruption, per-store repair so
`cache clear` stops being the only tool. A one-line decision — "rsconstruct
is single-process per project, enforced by one lock file" — followed by
deleting the multi-process scaffolding. A backend conformance test suite
(atomic overwrite, error-vs-absent, integrity of fetched descriptors) that
every `RemoteCache` impl must pass.

---

## 6. The cache model's core assumption — product ≡ invocation ≡ result ≡ positive output set — is false three ways

- **Batching breaks product ≡ invocation.** The cache is strictly
  per-product; execution is per-chunk. `execute_checker_batch` fans one
  exit status out to N products, so one bad file in a 50-file ruff chunk
  denies cache entries to 49 clean files on every build until it is fixed.
  Batching silently trades cache granularity for speed, and nothing in the
  types marks the trade (the per-file variant exists but is the minority
  path).
- **Outputs-as-inputs breaks product ≡ result.** A product's output is
  another product's (or its own) input. The entire eviction/recompute
  apparatus of §2 exists because the model assumed content is stable for
  one run and the build itself violates that assumption by writing files.
- **Trees are positive-only manifests.** A tree descriptor records what
  should exist, never what should be absent. Restore writes entries and
  deletes nothing; deletion lives only on the build path
  (`previous_tree_paths` before execution). So build-then-build converges
  while restore-then-restore leaves orphans, and a shrunk output set
  survives forever once restored.

Each shortfall is being addressed (or slated) as a per-processor patch flag
— `output_depends_on_input_name` is the acknowledged fourth instance of the
same shape. The pattern says the model's notion of product identity is
narrower than reality; widening the model once beats patching it per
symptom.

---

## 7. The meta-finding: the duplication is an output, not a habit

The 2026-08-02 audit named the recurring failure pattern: *knowledge
duplicated across hand-synchronized places with nothing enforcing
agreement*. This review's addition is causal: **that duplication is
generated, not chosen.** §1 generates it spatially — with no schema, every
new processor mints fresh hand-synced copies across 14 sites. §2 generates
it temporally — with no materialized facts, every phase re-derives the same
values and the copies must be kept in agreement by comments. The house
style then absorbs the structural pressure with the two tools it trusts,
comments and tests: `display.rs` was created to fix the `graph → cli`
inversion (which still exists, and `cli` re-exports the types straight
back); `phases.rs` is a 39-line cycle-breaker that re-enters its own cycle;
`build_context.rs` documents its own escape-hatch fields ("has no route to
the full Config"); `config/tests.rs` polices by hand what a schema would
make impossible. Each absorption is locally reasonable. Collectively the
module count grows while the topology stays tangled.

Also worth stating plainly: two of the project's rules — **no macros** and
**never create dummy instances** — are direct causes of §1 and §3
respectively. They may still be the right rules; but the trade should be
made with the price on the table, because right now the price is ~200 lines
of parallel hand lists, ~150 lines of policing tests, three match tables,
and a structurally second-class plugin system.

## Leverage ranking

1. **§1 — the processor schema split.** Removes the *generator* of the
   drift-bug class, unties the `config↔registries↔processors` knot, and is
   the prerequisite for any workspace split. Interim step available today
   at near-zero cost: the processor-level completeness test.
2. **§2, first slice — materialize `ResolvedInputs`; make `CacheKey`
   completeness un-forgeable.** Removes the generator of the
   cache-correctness class (the five-instant recompute, the eviction
   protocol, the deletion-by-stale-checksum edge). One written page —
   "which checksum keys a descriptor, and at which instant" — before any
   code.
3. **§4 — `Command` trait + injectable `Builder` + data/presenter split.**
   Not the deepest flaw, but the enabler: it is what makes §1 and §2 safe
   to execute, because it finally makes the command layer unit-testable
   (5 tests vs 31 entry points today).
4. **§3 — capability descriptor / synthetic Lua plugin entry.** Contained;
   closes a real cache-correctness hole for the only user extension path.
5. **§5 — persistence `Store` trait + single-process decision + backend
   conformance suite.** Contained; mostly subtraction.
6. **§6 — widen the cache model** (per-file batch results, negative space
   in trees) — do after §2, which supplies the vocabulary it needs.
