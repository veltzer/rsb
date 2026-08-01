# Distributed Execution

This document explores what distributed execution would mean for RSConstruct —
the problems it solves, the problems it creates, how other build tools approach
it, and what a design might look like.

## What distributed execution means

Today RSConstruct runs all products on the local machine, optionally in parallel
across multiple cores (`-j`). Distributed execution means offloading individual
products to remote workers — other machines on a network — so that the build
exploits more CPU than any single machine has.

This is distinct from remote caching (which RSConstruct already has). Remote
caching avoids re-running a product whose result was already computed by someone
else. Distributed execution runs products remotely even when no cached result
exists. The two features compose: a distributed build that also has remote
caching can share results across runs and across users.

## The problems it solves

- **Slow builds on large codebases.** When thousands of C files need checking
  or hundreds of PDFs need rendering, a single machine is the bottleneck even
  with `-j`. A cluster of workers can run all of them truly in parallel.
- **CI latency.** CI machines are often single-core or have limited parallelism.
  Distributing work across a pool of CI agents cuts wall-clock time.
- **Memory pressure.** Some tools (Chromium, LibreOffice, heavy linters) are
  memory-hungry. Spreading them across machines avoids OOM conditions.

## The problems it creates

### Input availability

Every product needs its inputs on the worker. For a checker that reads a single
source file, this means uploading that file to the worker (or having it available
via a shared filesystem). For a generator with many `dep_inputs`, it may mean
uploading dozens of files. This is a non-trivial data transfer problem.

The content-addressed object store already solves this at the output side —
outputs are stored by SHA-256. The same mechanism can serve inputs: if the
worker has a local object store, the coordinator only needs to send checksums,
and the worker fetches missing objects from the remote cache. Products whose
inputs are already cached require zero transfer.

### Output collection

After execution, the worker's outputs must be pushed back to the coordinator (or
directly to the remote cache) so local build phases and downstream products can
use them. This is essentially the existing remote cache push path.

### Hermeticity

Distributed workers only produce correct results if builds are hermetic — the
product's output depends only on its declared inputs, not on ambient machine
state (installed tools, environment variables, filesystem layout). RSConstruct
does not enforce hermeticity today. A worker with a different version of `ruff`
or `cppcheck` than the local machine will produce different results.

This is the hardest problem. Options:
- **Ignore it** — document that workers must have identical tool versions;
  use tool locking (`rsconstruct tools lock`) to detect divergence.
- **Containers** — run each product in a container image that includes all
  required tools. Bazel and BuildBuddy do this. Heavy but correct.
- **Nix/flakes** — pin tools via Nix derivations on all workers. Correct but
  requires Nix infrastructure.

### Scheduling and load balancing

Which products go to which worker? A central coordinator must:
1. Know the graph (dependency order).
2. Dispatch products whose dependencies are already satisfied.
3. Avoid overloading any single worker.
4. Handle worker failure (retry on another worker).

This is a distributed systems problem. Even a simple greedy scheduler requires
a reliable heartbeat, a work queue, and failure detection.

### Latency overhead

For fast products (a Python lint check on a 50-line file takes ~50ms), the
overhead of serializing inputs, sending them over the network, waiting for the
worker, and receiving results can exceed the actual execution time. Distributed
execution only pays off for products that take seconds or more, or when there
are so many products that local parallelism is saturated.

## How other tools do it

### Bazel (Remote Execution API)

Bazel defines the [Remote Execution API](https://github.com/bazelbuild/remote-apis)
(REAPI), a gRPC protocol for distributed execution. Workers implement the
`Execution` service; the coordinator submits `Action` objects (a command +
input digest tree). Workers fetch inputs from a Content Addressable Storage
(CAS) service, execute the action, and push outputs back to CAS.

Strengths: hermetic by design (actions are pure functions of their inputs),
well-specified protocol, many implementations (BuildBuddy, EngFlow, NativeLink,
self-hosted `buildfarm`).

Weaknesses: requires all actions to be declared with precise input sets;
dynamic dependencies (header includes discovered at compile time) need special
handling; heavy infrastructure to stand up.

RSConstruct's object store is conceptually similar to CAS. The `Product` struct
already declares all inputs explicitly. Implementing REAPI would make
RSConstruct compatible with the existing Bazel remote execution ecosystem
without building a proprietary scheduler.

### distcc

`distcc` distributes C/C++ compilation by intercepting `gcc`/`clang` invocations
and forwarding the preprocessed source to a pool of workers. It works at the
invocation level, not the build graph level — the local machine still runs the
build tool (make/ninja) and distcc is transparent to it.

Strengths: simple, no build tool integration required, widely deployed.

Weaknesses: only works for compilation (not linters, generators, etc.);
requires preprocessing locally (partial hermeticity); no caching.

### Incredibuild / Xtensa

Commercial tools that intercept process spawning at the OS level (Windows job
objects, Linux `LD_PRELOAD`) to virtualize and distribute arbitrary commands.
No build tool integration required; any tool that runs a subprocess can be
distributed.

Strengths: transparent to the build tool; works with any compiler or tool.

Weaknesses: proprietary; expensive; the OS-level interception is fragile.

### Pants / Buck2

Both use a daemon-based architecture with a local scheduler that knows the full
build graph. Distributed execution is an extension of local execution — the
scheduler dispatches actions to remote workers using REAPI or a proprietary
protocol. Input digests and output digests flow through a central CAS.

Pants calls this "remote execution"; Buck2 calls it "remote actions". Both
require the build rules to declare all inputs precisely (no dynamic deps).

### Ninja + a distributed wrapper

Some teams wrap Ninja with distributed backends (`ninja-build` + `icecc`,
`ninja` + `sccache`, or `ninja` + a custom scheduler). The wrapper intercepts
`compiler` invocations from the Ninja process. This is similar to the distcc
approach but can handle caching (sccache) alongside distribution.

## A possible design for RSConstruct

A minimal distributed execution design that fits RSConstruct's architecture:

### 1. Worker protocol

Workers expose a simple HTTP API:

```
POST /execute
  body: { product_id, command, args, input_checksums: {path: sha256, ...} }
  response: { exit_code, stdout, stderr, output_checksums: {path: sha256, ...} }
```

Before executing, the worker fetches any inputs it doesn't already have from
the shared remote cache. After executing, it pushes outputs to the remote cache
and returns their checksums.

### 2. Input availability via shared cache

The coordinator (local RSConstruct) ensures all inputs are in the remote cache
before dispatching a product to a worker. For source files, this means uploading
them once at build start. For intermediate outputs (products that are inputs to
other products), they flow through the cache automatically — the producer pushes
to remote, the consumer fetches from remote.

This avoids a separate "input upload" step for most products: source files are
small and stable; once uploaded they stay cached across builds.

### 3. Coordinator changes

The executor's product dispatch loop currently runs products locally. With
distributed execution:

1. Each dispatchable product is classified as local or remote based on a
   configurable predicate (e.g., processor type, estimated duration, worker
   availability).
2. Remote products are submitted to a work queue.
3. A pool of worker connections consumes the queue, tracking in-flight products.
4. When a remote product completes, its outputs are pulled from cache and the
   downstream products are unblocked.

The dependency graph and topological sort are unchanged — distribution is purely
an execution-layer concern.

### 4. Hermeticity via tool locking

Without containers, workers must have the same tool versions as the local
machine. `rsconstruct tools lock` already records tool version hashes.
Distributed execution should verify that each worker's tool hashes match the
lock file before accepting products of that type. A worker with a mismatched
`ruff` version refuses `ruff` products and logs a warning.

### 5. What stays local

Some products cannot or should not be distributed:

- Always-rebuild products (e.g., timestamp generators) — would need a
  per-processor opt-out from caching, which does not exist today.
- Products that depend on the local filesystem state beyond declared inputs
  (e.g., `git log` style operations).
- Creators that manage local directories (`npm install`, `cargo build`) —
  their outputs are directory trees, not files, and their side effects are
  local.
- Products faster than the round-trip overhead (most lint checks on small
  files).

A `distributed = false` config field (analogous to `enabled`) would let users
pin specific processors to local execution.

## Current status

Not implemented. RSConstruct runs all products locally. Remote caching
(push/pull of outputs) is the only cross-machine feature today.

The design above is a sketch for future consideration. The most natural first
step would be implementing a minimal REAPI-compatible worker, since that would
make RSConstruct interoperable with existing distributed build infrastructure
(BuildBuddy, EngFlow, self-hosted buildfarm) without requiring RSConstruct-
specific worker deployments.
