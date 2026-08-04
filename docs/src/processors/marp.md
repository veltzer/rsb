# Marp Processor

## Purpose

Converts Markdown slides to PDF, PPTX, or HTML using [Marp](https://marp.app/).

## How It Works

Discovers `.md` files in the project and runs `marp` on each file, generating
output in the configured formats. Each format produces a separate output file.

Each marp invocation spawns a headless Chromium browser instance via Puppeteer
to render the slides. This makes marp significantly more resource-intensive than
typical processors — see [Concurrency limiting](#concurrency-limiting) below.

## Source Files

- Input: `**/*.md`
- Output: `out/marp/{format}/{relative_path}.{format}`

## Configuration

```toml
[processor.marp]
command = "marp"                       # The marp command to run
formats = ["pdf"]                      # Output formats (pdf, pptx, html)
args = ["--html", "--allow-local-files"]  # Additional arguments to pass to marp
output_dir = "out/marp"                # Output directory
dep_inputs = []                      # Additional files that trigger rebuilds when changed
max_jobs = 2                           # Limit concurrent marp instances (each spawns Chromium)
```

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `command` | string | `"marp"` | The marp executable to run |
| `formats` | string[] | `["pdf"]` | Output formats to generate (`pdf`, `pptx`, `html`) |
| `args` | string[] | `["--html", "--allow-local-files"]` | Extra arguments passed to marp |
| `output_dir` | string | `"out/marp"` | Base output directory |
| `dep_inputs` | string[] | `[]` | Extra files whose changes trigger rebuilds |
| `max_jobs` | integer | none | Max concurrent marp processes. See [Concurrency limiting](#concurrency-limiting). |

## Concurrency Limiting

Each marp invocation launches a full headless Chromium browser process, which
consumes hundreds of megabytes of RAM. When running parallel builds with `-j N`,
too many simultaneous Chromium instances cause resource exhaustion and
non-deterministic crashes:

```
TargetCloseError: Protocol error (Target.setDiscoverTargets): Target closed
```

Use `max_jobs` to limit how many marp processes run concurrently, independent of
the global `-j` setting. For example, with `-j 20` and `max_jobs = 2`, at most
2 Chromium instances will be alive at once while other processors still use the
full 20 threads:

```toml
[processor.marp]
formats = ["pdf"]
max_jobs = 2
```

**Recommended value:** `2`. A value of 4 may work on machines with plenty of RAM
but has been observed to produce occasional failures on large projects (700+ slides).
Without `max_jobs`, the global `-j` value applies, which typically causes crashes
at higher parallelism levels.

## Batch Support

Each input file is processed individually, producing its own output file.

## Temporary Files

Marp creates temporary Chromium profile directories (`marp-cli-*`) in `/tmp` for
each invocation. RSConstruct automatically cleans these up after each marp process
completes, since marp itself does not delete them.

## Clean behavior

This processor is a Generator — `rsconstruct clean outputs` removes each declared output file individually with no directory recursion. After all per-product cleans complete, the orchestrator removes any parent directories that are now empty. Pass `--no-empty-dirs` to keep them. See [Clean behavior](../processors.md#clean-behavior) and [`rsconstruct clean`](../commands.md#rsconstruct-clean).
