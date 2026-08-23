# Download Policy

## Rule

Every network download rsconstruct performs MUST go through `src/download.rs`.
Call sites MUST NOT invoke `curl` directly, and MUST NOT call `ureq` without
wrapping the call in `download::with_retry`. New code containing a naked
`Command::new("curl")` WILL be rejected in code review.

The three entry points are:

| Helper                              | Use for                                                         |
| ----------------------------------- | --------------------------------------------------------------- |
| `download::curl_argv(url, dest)`    | Downloading a URL to a file. Returns argv; the caller executes it. |
| `download::apply_retry_args(&mut cmd)` | A curl `Command` needing flags this module does not own (`--head`, `-X PUT`, `-w %{http_code}`). |
| `download::with_retry(closure)`     | HTTP performed in-process via `ureq`.                            |

## Why

### 1. `curl` does not retry, and neither does `ureq`

Neither tool retries by default. `curl` needs explicit `--retry` flags;
`ureq` has connection pooling, which is easy to mistake for retry, but a
transport failure surfaces straight to the caller. A naked call at either
layer is a call with no resilience at all — and nothing about the call site
makes that visible to a reader.

### 2. One place to change policy

Before this module there were four independent `curl -fsSL` invocations
across `tools.rs` and `remote_cache.rs`, plus two bare `ureq` calls. Changing
retry behavior, adding a timeout, or setting a header meant finding all six
and editing them consistently. Anything less than all six produces a
codebase where robustness depends on which code path you happen to hit.

### 3. Preview must match execution

`tools install` renders its steps for preview (`describe_binary`) and
separately executes them (`run_binary`). Those were two hand-written argv
literals that had to be kept in sync by hand. Both now call `curl_argv`, so
the preview cannot drift from what actually runs.

## What the retry is and is not for

This policy exists for **connection-level failures**: a connection reset,
refused, or dropped during handshake. The motivating incident was a
`tools install` run that died on:

```
curl: (35) Recv failure: Connection reset by peer
FAILED [binary] rumdl, taplo: curl -fsSL -o /tmp/taplo.dl https://... exited with code 35
```

The connection died about half a second in. Re-running the identical job
minutes later succeeded in 43 seconds. That is the failure shape a retry
fixes: the request never got started, so trying again costs almost nothing
and usually works.

It is explicitly **not** for a slow-but-progressing transfer. The
2026-08-19 incidents involved a mirror serving at roughly 60 KB/s — slow,
but making progress. Against that, a retry accomplishes nothing (each
attempt is equally slow) and a total-transfer timeout is actively harmful:
one such timeout killed a healthy release build. This is why:

- **`--connect-timeout` is set, `--max-time` is not.** Bounding the
  handshake catches a dead endpoint. Bounding the whole transfer kills a
  working download of a large asset over a slow link. A unit test in
  `src/download.rs` asserts `--max-time` stays absent.
- **The attempt count is 3, not 10.** Three attempts absorb a transient
  reset. A larger number just means a genuinely broken endpoint takes
  longer to report a failure that was never going to resolve.

The remedy for a *degraded runner* remains what it was: cancel and re-run on
fresh hardware, in the consuming repo's CI. This module does not change that
and cannot.

## Flags, and why each is there

```
--retry 3 --retry-delay 2 --retry-connrefused --retry-all-errors --connect-timeout 30
```

`--retry-connrefused` and `--retry-all-errors` carry the weight.  Plain
`--retry` covers HTTP 5xx responses and a narrow set of transport errors —
it does **not** cover a connection reset during handshake, which is exit
code 35 and precisely the case this policy addresses. Without those two
flags the retry count would have made no difference to the incident above.

## Adding a new download

1. **Downloading a URL to a file?** Use `curl_argv`. Do not write a `curl`
   argv by hand.
2. **Need curl flags the module does not own?** Build the `Command`, then
   call `apply_retry_args` on it before adding your flags.
3. **Fetching in-process with `ureq`?** Wrap the call in `with_retry`, and
   keep the whole request-and-read sequence inside the closure — a retry
   that re-reads a consumed body is not a retry.
4. **Tempted to add a timeout knob?** Read "What the retry is and is not
   for" above first. A total-transfer timeout has already broken a release
   build in this project once.

Any new download call site should carry a comment pointing at this document,
as the `ureq` sites in `src/tools.rs` and `src/webcache.rs` do.

## Audit

```bash
rg -n 'Command::new\("curl"\)' src/
rg -n 'ureq::' src/
```

Every `Command::new("curl")` match must be followed by an
`apply_retry_args` call, and every `ureq::` match must sit inside a
`with_retry` closure. As of this writing that is three curl sites (all in
`src/remote_cache.rs`) and two `ureq` sites (`src/tools.rs`,
`src/webcache.rs`). The binary installer in `src/tools.rs` builds its argv
through `curl_argv` and so does not spawn curl by name at all.
